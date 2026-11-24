//! OEB/XHTML -> Textile-formatted plain text.
//!
//! Port of `old_src/src/calibre/ebooks/txt/textileml.py`'s
//! `TextileMLizer`. Same tree-walk shape as `crate::txt::markdownml::MarkdownMlizer`
//! -- see that module's docs for why this isn't built on
//! `crate::htmlz::oeb2html::Oeb2Html`. The same reasoning applies here,
//! with one refinement: unlike markdown output, Textile output *does*
//! read `id` attributes (`check_id_tag`, Textile's `(#id)` attribute
//! syntax), so this port uses each element's raw `id` value directly
//! rather than reimplementing `Oeb2Html`'s cross-file id renumbering.
//! That means two spine files that happen to share a literal `id`
//! collide in the output the way they never would with
//! `rewrite_ids`'s renumbering -- a deliberate, narrow scope limitation
//! for a converter whose Python original is itself single-pass and
//! whose id-cleanup pass (`tidy_up`'s dangling-anchor stripping) only
//! ever looks at a flat set of strings anyway.
//!
//! # Reuse
//!
//! - `unsmarten_punctuation` wires directly to `crate::textile::unsmarten::unsmarten`
//!   (issue #53), matching `self.opts.unsmarten_punctuation: txt =
//!   unsmarten(txt)`.
//! - `check_padding`'s length math uses `crate::oeb::transforms::flatcss::unit_convert`,
//!   the real port of `calibre.ebooks.unit_convert`, rather than a
//!   second hand-rolled copy.
//!
//! # Where `ResolvedStyle` falls short
//!
//! `dump_text` also wants `color`, `background`, `text-align`,
//! `vertical-align`, `font-variant`, `padding-left`/`-right`,
//! `margin-left`/`-right`/`-bottom` -- none of which
//! `crate::oeb::stylizer::ResolvedStyle` carries as first-class fields.
//! Each is pulled out of `ResolvedStyle::css_text` (the element's own
//! declared `style="..."` text) via [`inline_style_prop`], mirroring
//! the precedent `pml/pmlml.rs` and `rb/rbml.rs` already set for
//! `text-align`. `check_padding`'s percentage case (`style.width`, the
//! *page* width in Python, used for `%`-based padding/margin) has no
//! equivalent here at all -- there is no page-layout concept in this
//! crate's `StyleProvider` -- so a `%` padding/margin resolves to 0pt,
//! same documented gap `pml/pmlml.rs` already carries for its own
//! `margin-left` handling.
//!
//! `stylizer.profile.dpi`/`fbase` (used by `check_padding` and the
//! margin-driven soft-scene-break math) have no output-profile system
//! backing them either; [`TextileMlizer::new`] defaults to 96dpi/12pt,
//! matching the `Stylizer::new(96.0, 12.0)` convention used throughout
//! this crate's own tests, and [`TextileMlizer::with_profile`] lets a
//! caller override it.
//!
//! # Preserved upstream quirks
//!
//! - **The `pre` close-tag sentinel never matches its own reset
//!   check.** `dump_text` pushes `tags.append('pre\n')` (five
//!   characters, with a trailing newline) when opening a `<pre>`, but
//!   the closing loop's special-case is `if t == 'pre':` (bare, three
//!   characters) -- `"pre\n" != "pre"`, so that branch never fires.
//!   `self.in_pre` is therefore *never reset to `False` by the closing
//!   loop*, and the literal text `"pre\n"` leaks into the output where
//!   the (dead) special-casing was presumably meant to suppress it.
//!   Once any `<pre>` closes, every later element's own/tail text skips
//!   `remove_newlines`/`prepare_string_for_textile` for the rest of the
//!   document (since both are gated on `if not self.in_pre`), and any
//!   later `<code>` renders as a `bc.` block instead of `@inline@`.
//!   Verified against the exact `tags.append(...)` call sites for every
//!   tag this closing check names (`'pre'`, `'ul'`, `'ol'`, `'li'`,
//!   `'table'`): only `'ul'`/`'ol'` are ever actually pushed under
//!   their bare names (`tags.append(tag)`); `'li'`/`'table'`/`'td'`/`'th'`/`'dd'`
//!   push `''`, so those two tuple members are dead too. Ported exactly
//!   -- the closing loop's string comparisons are reproduced verbatim,
//!   not "fixed" to match what the special-casing was clearly meant to
//!   do.

use std::sync::OnceLock;

use regex::Regex;
use roxmltree::{Document, Node};

use crate::oeb::book::OEBBook;
pub use crate::oeb::stylizer::{ResolvedStyle, StyleProvider, TagStylizer};
use crate::oeb::transforms::flatcss::unit_convert;
use crate::textile::unsmarten::unsmarten;

const XHTML_NS: &str = "http://www.w3.org/1999/xhtml";

/// Options `extract_content` reads. Port of the subset of `opts`
/// `textileml.py` actually touches.
#[derive(Debug, Clone, Default)]
pub struct TextileMlOptions {
    pub keep_links: bool,
    pub keep_image_references: bool,
    pub keep_color: bool,
    pub unsmarten_punctuation: bool,
}

#[derive(Debug, Clone)]
struct ListState {
    name: String,
}

/// Port of `calibre.ebooks.txt.textileml.TextileMLizer`.
#[derive(Debug)]
pub struct TextileMlizer {
    in_pre: bool,
    our_links: Vec<String>,
    our_ids: Vec<String>,
    in_a_link: bool,
    id_no_text: String,
    style_embed: Vec<char>,
    style_bold: bool,
    style_italic: bool,
    style_under: bool,
    style_strike: bool,
    style_smallcap: bool,
    list: Vec<ListState>,
    remove_space_after_newline: bool,
    /// Stands in for `stylizer.profile.dpi`. See the module docs.
    dpi: f64,
    /// Stands in for `stylizer.profile.fbase`. See the module docs.
    fbase: f64,
}

impl Default for TextileMlizer {
    fn default() -> Self {
        TextileMlizer {
            in_pre: false,
            our_links: Vec::new(),
            our_ids: Vec::new(),
            in_a_link: false,
            id_no_text: String::new(),
            style_embed: Vec::new(),
            style_bold: false,
            style_italic: false,
            style_under: false,
            style_strike: false,
            style_smallcap: false,
            list: Vec::new(),
            remove_space_after_newline: false,
            dpi: 96.0,
            fbase: 12.0,
        }
    }
}

impl TextileMlizer {
    /// Port of `MAX_EM`.
    const MAX_EM: i64 = 10;

    pub fn new() -> Self {
        Self::default()
    }

    /// Like [`TextileMlizer::new`], but with an explicit
    /// dpi/base-font-size pair instead of the 96/12 default. See the
    /// module docs.
    pub fn with_profile(dpi: f64, fbase: f64) -> Self {
        TextileMlizer {
            dpi,
            fbase,
            ..Self::default()
        }
    }

    /// Port of `extract_content` + `mlize_spine` + `tidy_up`.
    pub fn extract_content(
        &mut self,
        oeb: &OEBBook,
        opts: &TextileMlOptions,
        stylizer: &dyn StyleProvider,
    ) -> String {
        let (dpi, fbase) = (self.dpi, self.fbase);
        *self = TextileMlizer {
            dpi,
            fbase,
            ..TextileMlizer::default()
        };

        let mut txt = self.mlize_spine(oeb, opts, stylizer);
        if opts.unsmarten_punctuation {
            txt = unsmarten(&txt);
        }
        self.tidy_up(&txt, opts)
    }

    fn mlize_spine(
        &mut self,
        oeb: &OEBBook,
        opts: &TextileMlOptions,
        stylizer: &dyn StyleProvider,
    ) -> String {
        let mut output = String::new();
        for spine_item in &oeb.spine.items {
            let Some(item) = oeb.manifest.get_by_id(&spine_item.idref) else {
                continue;
            };
            let Ok(raw) = oeb.container.read(&item.href) else {
                continue;
            };
            let content = String::from_utf8_lossy(&raw).into_owned();
            let Ok(doc) = Document::parse(&content) else {
                continue;
            };
            if let Some(body) = find_body(&doc) {
                let mut out = Vec::new();
                self.dump_text(body, opts, stylizer, &mut out);
                output.push_str(&out.concat());
            }
            output.push_str("\n\n");
        }
        output
    }

    /// Port of `remove_newlines`.
    fn remove_newlines(&mut self, text: &str) -> String {
        static MULTI_SPACE: OnceLock<Regex> = OnceLock::new();
        static TABS: OnceLock<Regex> = OnceLock::new();
        static LEADING_SPACES: OnceLock<Regex> = OnceLock::new();
        let multi_space = MULTI_SPACE.get_or_init(|| Regex::new(r"[ ]{2,}").expect("regex"));
        let tabs = TABS.get_or_init(|| Regex::new(r"\t+").expect("regex"));
        let leading_spaces = LEADING_SPACES.get_or_init(|| Regex::new(r"^ +").expect("regex"));

        let mut text = text.replace("\r\n", " ").replace(['\n', '\r'], " ");
        text = multi_space.replace_all(&text, " ").into_owned();
        text = tabs.replace_all(&text, "").into_owned();
        if self.remove_space_after_newline {
            text = leading_spaces.replace_all(&text, "").into_owned();
            self.remove_space_after_newline = false;
        }
        text
    }

    /// Port of `check_styles`.
    fn check_styles(&self, style: &ResolvedStyle, opts: &TextileMlOptions) -> String {
        let mut txt = String::from("{");
        if opts.keep_color {
            if let Some(color) = inline_style_prop(&style.css_text, "color") {
                if color != "black" {
                    txt.push_str(&format!("color:{color};"));
                }
            }
            if let Some(bg) = inline_style_prop(&style.css_text, "background") {
                txt.push_str(&format!("background:{bg};"));
            }
        }
        txt.push('}');
        if txt == "{}" {
            String::new()
        } else {
            txt
        }
    }

    /// Port of `check_halign`.
    fn check_halign(style: &ResolvedStyle) -> &'static str {
        match inline_style_prop(&style.css_text, "text-align").as_deref() {
            Some("left") => "<",
            Some("justify") => "<>",
            Some("center") => "=",
            Some("right") => ">",
            _ => "",
        }
    }

    /// Port of `check_valign`.
    fn check_valign(style: &ResolvedStyle) -> &'static str {
        match inline_style_prop(&style.css_text, "vertical-align").as_deref() {
            Some("top") => "^",
            Some("bottom") => "~",
            _ => "",
        }
    }

    /// Port of `check_padding`. See the module docs for the `%`-base
    /// and dpi/fbase caveats.
    fn check_padding(&self, style: &ResolvedStyle) -> String {
        let mut txt = String::new();

        let conv = |prop: &str| -> f64 {
            inline_style_prop(&style.css_text, prop)
                .filter(|v| v != "auto")
                .and_then(|v| unit_convert(&v, 0.0, style.font_size, self.dpi, 12.0))
                .unwrap_or(0.0)
        };

        let left = conv("margin-left") + conv("padding-left");
        let em_left = ((left / self.fbase).round() as i64).min(Self::MAX_EM);
        if em_left >= 1 {
            txt.push_str(&"(".repeat(em_left as usize));
        }

        let right = conv("margin-right") + conv("padding-right");
        let em_right = ((right / self.fbase).round() as i64).min(Self::MAX_EM);
        if em_right >= 1 {
            txt.push_str(&")".repeat(em_right as usize));
        }

        txt
    }

    /// Port of `check_id_tag`.
    fn check_id_tag(&mut self, id_attr: Option<&str>) -> String {
        match id_attr {
            Some(id) => {
                self.our_ids.push(format!("#{id}"));
                self.id_no_text = "\u{a0}".to_string();
                format!("(#{id})")
            }
            None => String::new(),
        }
    }

    /// Port of `build_block`.
    fn build_block(
        &mut self,
        tag: &str,
        style: &ResolvedStyle,
        id_attr: Option<&str>,
        opts: &TextileMlOptions,
    ) -> String {
        let mut txt = format!("\n{tag}");
        if opts.keep_links {
            txt.push_str(&self.check_id_tag(id_attr));
        }
        txt.push_str(&self.check_padding(style));
        txt.push_str(Self::check_halign(style));
        txt.push_str(&self.check_styles(style, opts));
        txt
    }

    /// Port of `prepare_string_for_textile`.
    fn prepare_string_for_textile(txt: &str) -> String {
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| {
            Regex::new(r#"(\s([*&_+\-~@%|]|\?{2})\S)|(\S([*&_+\-~@%|]|\?{2})\s)"#).expect("regex")
        });
        if re.is_match(txt) {
            format!(" =={txt}== ")
        } else {
            txt.to_string()
        }
    }

    /// Port of `dump_text`.
    fn dump_text(
        &mut self,
        elem: Node,
        opts: &TextileMlOptions,
        stylizer: &dyn StyleProvider,
        out: &mut Vec<String>,
    ) {
        if !elem.is_element() {
            return;
        }
        let ns = elem.tag_name().namespace();
        if !(ns.is_none() || ns == Some(XHTML_NS)) {
            if let Some(tail) = tail_text(elem) {
                out.push(tail);
            }
            return;
        }

        let style = stylizer.style(elem);
        let mut tag = elem.tag_name().name().to_string();
        let mut tags: Vec<String> = Vec::new();

        if matches!(
            style.display.as_str(),
            "none" | "oeb-page-head" | "oeb-page-foot"
        ) || style.visibility == "hidden"
        {
            if let Some(tail) = tail_text(elem) {
                out.push(tail);
            }
            return;
        }

        // Soft scene breaks (top).
        let top_ems = ((style.margin_top / style.font_size).round() as i64 - 1).min(Self::MAX_EM);
        if top_ems >= 1 {
            out.push("\n\n\u{a0}".repeat(top_ems as usize));
        }

        let is_heading = matches!(tag.as_str(), "h1" | "h2" | "h3" | "h4" | "h5" | "h6");
        if is_heading || tag == "p" || tag == "div" {
            if tag == "div" {
                tag = "p".to_string();
            }
            let block = self.build_block(&tag, &style, elem.attribute("id"), opts);
            out.push(block);
            out.push(". ".to_string());
            tags.push("\n".to_string());
        }

        let not_heading_or_cite = !matches!(
            tag.as_str(),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "cite"
        );
        if (style.font_style == "italic" || matches!(tag.as_str(), "i" | "em"))
            && not_heading_or_cite
            && !self.style_italic
        {
            if self.in_a_link {
                out.push("_".to_string());
                tags.push("_".to_string());
            } else {
                out.push("[_".to_string());
                tags.push("_]".to_string());
            }
            self.style_embed.push('_');
            self.style_italic = true;
        }
        let not_heading_or_th =
            !matches!(tag.as_str(), "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "th");
        if (matches!(style.font_weight.as_str(), "bold" | "bolder")
            || matches!(tag.as_str(), "b" | "strong"))
            && not_heading_or_th
            && !self.style_bold
        {
            if self.in_a_link {
                out.push("*".to_string());
                tags.push("*".to_string());
            } else {
                out.push("[*".to_string());
                tags.push("*]".to_string());
            }
            self.style_embed.push('*');
            self.style_bold = true;
        }
        let text_decoration_underline =
            style.text_decoration == "underline" || matches!(tag.as_str(), "u" | "ins");
        if text_decoration_underline && tag != "a" && !self.style_under {
            out.push("[+".to_string());
            tags.push("+]".to_string());
            self.style_embed.push('+');
            self.style_under = true;
        }
        let text_decoration_strike = style.text_decoration == "line-through"
            || matches!(tag.as_str(), "strike" | "del" | "s");
        if text_decoration_strike && !self.style_strike {
            out.push("[-".to_string());
            tags.push("-]".to_string());
            self.style_embed.push('-');
            self.style_strike = true;
        }

        if tag == "br" {
            for c in self.style_embed.iter().rev() {
                out.push(c.to_string());
            }
            out.push("\n".to_string());
            for c in &self.style_embed {
                out.push(c.to_string());
            }
            tags.push(String::new());
            self.remove_space_after_newline = true;
        }

        match tag.as_str() {
            "blockquote" => {
                out.push("\nbq. ".to_string());
                tags.push("\n".to_string());
            }
            "abbr" | "acronym" => {
                out.push(String::new());
                let title = elem.attribute("title").unwrap_or("");
                tags.push(format!("({title})"));
            }
            "sup" => {
                out.push("^".to_string());
                tags.push("^".to_string());
            }
            "sub" => {
                out.push("~".to_string());
                tags.push("~".to_string());
            }
            "code" => {
                if self.in_pre {
                    out.push("\nbc. ".to_string());
                    tags.push(String::new());
                } else {
                    out.push("@".to_string());
                    tags.push("@".to_string());
                }
            }
            "cite" => {
                out.push("??".to_string());
                tags.push("??".to_string());
            }
            "hr" => {
                out.push("\n***".to_string());
                tags.push("\n".to_string());
            }
            "pre" => {
                self.in_pre = true;
                out.push("\npre. ".to_string());
                // See the module docs: this sentinel never matches the
                // closing loop's `t == "pre"` reset check.
                tags.push("pre\n".to_string());
            }
            "a" => {
                if opts.keep_links {
                    if let Some(href) = elem.attribute("href") {
                        out.push("\"".to_string());
                        tags.push("a".to_string());
                        tags.push(format!("\":{href}"));
                        self.our_links.push(href.to_string());
                        if let Some(title) = elem.attribute("title") {
                            tags.push(format!("({title})"));
                        }
                        self.in_a_link = true;
                    } else {
                        out.push("%".to_string());
                        tags.push("%".to_string());
                    }
                }
            }
            "img" => {
                if opts.keep_image_references {
                    let mut txt = format!(
                        "!{}{}",
                        Self::check_halign(&style),
                        Self::check_valign(&style)
                    );
                    txt.push_str(elem.attribute("src").unwrap_or(""));
                    out.push(txt);
                    if let Some(alt) = elem.attribute("alt").filter(|a| !a.is_empty()) {
                        out.push(format!("({alt})"));
                    }
                    tags.push("!".to_string());
                }
            }
            "ol" | "ul" => {
                self.list.push(ListState { name: tag.clone() });
                out.push(String::new());
                tags.push(tag.clone());
            }
            "li" => {
                let name = self
                    .list
                    .last()
                    .map(|l| l.name.clone())
                    .unwrap_or_else(|| "ul".to_string());
                out.push("\n".to_string());
                let depth = self.list.len();
                if name == "ul" {
                    out.push(format!("{} ", "*".repeat(depth)));
                } else if name == "ol" {
                    out.push(format!("{} ", "#".repeat(depth)));
                }
                tags.push(String::new());
            }
            "dl" => {
                out.push("\n".to_string());
                tags.push(String::new());
            }
            "dt" => {
                out.push(String::new());
                tags.push("\n".to_string());
            }
            "dd" => {
                out.push("    ".to_string());
                tags.push(String::new());
            }
            "table" => {
                let mut txt = self.build_block("table", &style, elem.attribute("id"), opts);
                txt.push_str(". \n");
                if txt != "\ntable. \n" {
                    out.push(txt);
                } else {
                    out.push("\n".to_string());
                }
                tags.push(String::new());
            }
            "tr" => {
                let mut txt = self.build_block("", &style, elem.attribute("id"), opts);
                txt.push_str(". ");
                if txt != "\n. " {
                    out.push(txt.replace('\n', ""));
                }
                tags.push("|\n".to_string());
            }
            "td" => {
                out.push("|".to_string());
                let mut txt = format!(
                    "{}{}",
                    Self::check_halign(&style),
                    Self::check_valign(&style)
                );
                if let Some(colspan) = elem.attribute("colspan") {
                    txt.push('\\');
                    txt.push_str(colspan);
                }
                if let Some(rowspan) = elem.attribute("rowspan") {
                    txt.push('/');
                    txt.push_str(rowspan);
                }
                txt.push_str(&self.check_styles(&style, opts));
                if !txt.is_empty() {
                    out.push(format!("{txt}. "));
                }
                tags.push(String::new());
            }
            "th" => {
                out.push("|_. ".to_string());
                tags.push(String::new());
            }
            "span" => {
                if inline_style_prop(&style.css_text, "font-variant").as_deref()
                    == Some("small-caps")
                {
                    if !self.style_smallcap {
                        out.push("&".to_string());
                        tags.push("&".to_string());
                        self.style_smallcap = true;
                    }
                } else if !self.in_a_link {
                    let mut txt = String::from("%");
                    if opts.keep_links {
                        txt.push_str(&self.check_id_tag(elem.attribute("id")));
                        txt.push_str(&self.check_styles(&style, opts));
                    }
                    if txt != "%" {
                        out.push(txt);
                        tags.push("%".to_string());
                    }
                }
            }
            _ => {}
        }

        if opts.keep_links && elem.attribute("id").is_some() {
            let excluded = matches!(
                tag.as_str(),
                "body" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "p" | "span" | "table"
            );
            if !excluded {
                let id_tag = self.check_id_tag(elem.attribute("id"));
                out.push(id_tag);
            }
        }

        let excluded_for_style = matches!(
            tag.as_str(),
            "body"
                | "div"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "p"
                | "hr"
                | "a"
                | "img"
                | "span"
                | "table"
                | "tr"
                | "td"
        );
        if !excluded_for_style && !self.in_a_link {
            let styles = self.check_styles(&style, opts);
            out.push(styles);
        }

        if let Some(text) = own_text(elem) {
            let converted = if self.in_pre {
                text
            } else {
                let removed = self.remove_newlines(&text);
                Self::prepare_string_for_textile(&removed)
            };
            out.push(converted);
            self.id_no_text.clear();
        }

        for child in elem.children() {
            self.dump_text(child, opts, stylizer, out);
        }

        for t in tags.into_iter().rev() {
            if matches!(t.as_str(), "pre" | "ul" | "ol" | "li" | "table") {
                if t == "ul" || t == "ol" {
                    self.list.pop();
                    if self.list.is_empty() {
                        out.push("\n".to_string());
                    }
                }
                // "pre" is never actually pushed under this bare name
                // (see module docs), so `self.in_pre = false` here is
                // dead code upstream and is not replicated.
            } else {
                let mut t = t;
                if t == "a" {
                    self.in_a_link = false;
                    t = String::new();
                }
                out.push(std::mem::take(&mut self.id_no_text));
                match t.as_str() {
                    "*]" | "*" => self.style_bold = false,
                    "_]" | "_" => self.style_italic = false,
                    "+]" => self.style_under = false,
                    "-]" => self.style_strike = false,
                    "&" => self.style_smallcap = false,
                    _ => {}
                }
                if matches!(t.as_str(), "*]" | "_]" | "+]" | "-]" | "*" | "_") {
                    self.style_embed.pop();
                }
                out.push(t);
            }
        }

        // Soft scene breaks (bottom). See the module docs for why this
        // is pulled from `css_text` rather than a `ResolvedStyle`
        // field.
        if let Some(mb) = inline_style_prop(&style.css_text, "margin-bottom")
            .filter(|v| v != "auto")
            .and_then(|v| unit_convert(&v, 0.0, style.font_size, self.dpi, 12.0))
        {
            let ems = ((mb / style.font_size).round() as i64 - 1).min(Self::MAX_EM);
            if ems >= 1 {
                out.push("\n\n\u{a0}".repeat(ems as usize));
            }
        }

        if let Some(tail) = tail_text(elem) {
            let converted = if self.in_pre {
                tail
            } else {
                let removed = self.remove_newlines(&tail);
                Self::prepare_string_for_textile(&removed)
            };
            out.push(converted);
        }
    }

    /// Port of `check_escaping`, called from `tidy_up`.
    fn check_escaping(text: &str, tests: &[&str]) -> String {
        let mut text = text.to_string();
        for t in tests {
            if *t != "%" {
                if let Ok(re) = Regex::new(&format!(r"([^{t}|^\n]){t}\]\[{t}([^{t}])")) {
                    text = re.replace_all(&text, "$1$2").into_owned();
                }
                if let Ok(re) = Regex::new(&format!(r"([^{t}|^\n]){t}{t}([^{t}])")) {
                    text = re.replace_all(&text, "$1$2").into_owned();
                }
            }
            if let Ok(re) = Regex::new(&format!(
                r#"(\s|[*_'"])\[({t}[a-zA-Z0-9 '",.*_]+{t})\](\s|[*_'"?!,.])"#
            )) {
                text = re.replace_all(&text, "$1$2$3").into_owned();
            }
        }
        text
    }

    /// Port of `tidy_up`. Every static (non-`our_links`/`our_ids`)
    /// substitution here is real Textile-output cleanup with no
    /// lookaround, so plain `regex` covers it -- see the module docs
    /// for the pattern list.
    fn tidy_up(&self, text: &str, opts: &TextileMlOptions) -> String {
        let mut text = text.to_string();

        if opts.keep_links {
            for link in &self.our_links {
                if link.starts_with('#') && !self.our_ids.contains(link) {
                    if let Ok(re) = Regex::new(&format!("\"(.+)\":{link}(\\s)")) {
                        text = re.replace_all(&text, "$1$2").into_owned();
                    }
                }
            }
            for id in &self.our_ids {
                if !self.our_links.contains(id) {
                    let pat = format!("%?\\({id}\\)\u{a0}?%?");
                    if let Ok(re) = Regex::new(&pat) {
                        text = re.replace_all(&text, "").into_owned();
                    }
                }
            }
        }

        text = Self::check_escaping(&text, &[r"\*", "_", r"\*"]);

        macro_rules! re_sub {
            ($text:expr, $pat:expr, $repl:expr) => {{
                static RE: OnceLock<Regex> = OnceLock::new();
                let re = RE.get_or_init(|| Regex::new($pat).expect("regex"));
                re.replace_all(&$text, $repl).into_owned()
            }};
        }

        text = re_sub!(text, r"(\w)([~^]\w+[~^])", "$1[$2]");
        text = re_sub!(text, r"([~^]\w+[~^])(\w)", "[$1]$2");
        text = re_sub!(text, "%\u{a0}+", "%");
        text = text.replace("%%", "");
        text = re_sub!(text, r"%([_+*-]+)%", "$1");
        text = re_sub!(text, r" +\n", "\n");
        text = re_sub!(text, r"^\n+", "");
        text = re_sub!(text, r"\npre\.\n?\nbc\.", "\nbc.");
        text = re_sub!(text, r"\nbq\.\n?\np.*?\. ", "\nbq. ");
        text = re_sub!(text, r"\n{3}", "\n\np. \n\n");
        text = re_sub!(text, r"%\n(p[<>=]{1,2}\.|p\.)", "%\n\n$1");
        text = re_sub!(text, r"\n+ +%", " %");
        text = re_sub!(text, r"p[<>=]{1,2}\.\n\n?", "");
        text = re_sub!(text, r"\n(p.*\.)\n", "\n$1 \n\n");
        text = text.replace("\n\u{a0}", "\np. ");
        text = re_sub!(text, r"\np[<>=]{1,2}?\. \u{a0}", "\np. ");
        text = re_sub!(text, r"(^|\n)(p.*\. ?\n)(p.*\.)", "$1$3");
        text = re_sub!(text, r"\n(p\. \n)(p.*\.|h.*\.)", "\n$2");
        text = re_sub!(text, r" {2,}\|", " |");
        text = re_sub!(text, r"\np\.\n", "\np. \n");
        text = re_sub!(text, r" \n\n\n", " \n\n");

        text
    }
}

/// Extract one declaration's value out of an inline `style="..."` CSS
/// text. Mirrors `pml/pmlml.rs`'s helper of the same name.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::container::Container;
    use crate::oeb::manifest::ManifestItem;
    use crate::oeb::spine::SpineItem;
    use crate::oeb::stylizer::Stylizer as ConcreteStylizer;
    use anyhow::Result;
    use std::collections::HashMap as Map;

    #[derive(Default)]
    struct MemContainer(Map<String, Vec<u8>>);

    impl Container for MemContainer {
        fn read(&self, path: &str) -> Result<Vec<u8>> {
            self.0
                .get(path)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("no such part: {path}"))
        }
        fn write(&mut self, path: &str, data: &[u8]) -> Result<()> {
            self.0.insert(path.to_string(), data.to_vec());
            Ok(())
        }
        fn exists(&self, path: &str) -> bool {
            self.0.contains_key(path)
        }
        fn namelist(&self) -> Result<Vec<String>> {
            Ok(self.0.keys().cloned().collect())
        }
    }

    fn book(html: &str) -> OEBBook {
        let mut container = MemContainer::default();
        container
            .0
            .insert("index.html".to_string(), html.as_bytes().to_vec());
        let mut oeb = OEBBook::new(Box::new(container));
        oeb.manifest.items.insert(
            "item0".to_string(),
            ManifestItem::new("item0", "index.html", "application/xhtml+xml"),
        );
        oeb.spine.items.push(SpineItem::new("item0", true));
        oeb
    }

    fn convert(html: &str, opts: &TextileMlOptions) -> String {
        let oeb = book(html);
        TextileMlizer::new().extract_content(&oeb, opts, &TagStylizer)
    }

    #[test]
    fn headings_use_h_dot_prefix() {
        let html =
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><h1>Title</h1></body></html>"#;
        let out = convert(html, &TextileMlOptions::default());
        assert!(out.contains("h1. Title"), "{out}");
    }

    #[test]
    fn bold_and_italic_use_bracketed_markers() {
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>plain <b>bold</b> and <i>italic</i></p></body></html>"#;
        let out = convert(html, &TextileMlOptions::default());
        assert!(out.contains("bold"), "{out}");
        assert!(out.contains("italic"), "{out}");
    }

    #[test]
    fn blockquotes_use_bq_dot() {
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><blockquote><p>quoted</p></blockquote></body></html>"#;
        let out = convert(html, &TextileMlOptions::default());
        assert!(out.contains("bq."), "{out}");
    }

    #[test]
    fn links_use_quoted_colon_url_syntax_when_enabled() {
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p><a href="http://example.com/">link</a></p></body></html>"#;
        let opts = TextileMlOptions {
            keep_links: true,
            ..Default::default()
        };
        let out = convert(html, &opts);
        assert!(out.contains(":http://example.com/"), "{out}");
    }

    #[test]
    fn images_use_bang_syntax_when_enabled() {
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p><img src="a.png"/></p></body></html>"#;
        let opts = TextileMlOptions {
            keep_image_references: true,
            ..Default::default()
        };
        let out = convert(html, &opts);
        assert!(out.contains("!a.png!"), "{out}");
    }

    #[test]
    fn ordered_and_unordered_lists_use_hash_and_star_markers() {
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><ul><li>a</li></ul><ol><li>b</li></ol></body></html>"#;
        let out = convert(html, &TextileMlOptions::default());
        assert!(out.contains("* a"), "{out}");
        assert!(out.contains("# b"), "{out}");
    }

    #[test]
    fn tables_produce_pipe_delimited_rows() {
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><table><tr><td>a</td><td>b</td></tr></table></body></html>"#;
        let out = convert(html, &TextileMlOptions::default());
        assert!(out.contains('|'), "{out}");
        assert!(out.contains('a') && out.contains('b'), "{out}");
    }

    #[test]
    fn hidden_content_is_dropped() {
        // `TagStylizer` (what `convert()` uses) only knows tag-name
        // defaults, not inline `style="..."` -- it can't see
        // `display: none` here, so this needs the real `Stylizer`.
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>keep</p><p style="display: none">drop</p></body></html>"#;
        let oeb = book(html);
        let stylizer = ConcreteStylizer::new(96.0, 12.0);
        let out =
            TextileMlizer::new().extract_content(&oeb, &TextileMlOptions::default(), &stylizer);
        assert!(out.contains("keep"), "{out}");
        assert!(!out.contains("drop"), "{out}");
    }

    #[test]
    fn unsmarten_punctuation_runs_when_enabled() {
        let html =
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>&#162; cent</p></body></html>"#;
        let opts = TextileMlOptions {
            unsmarten_punctuation: true,
            ..Default::default()
        };
        let out = convert(html, &opts);
        assert!(out.contains("{c\\}"), "{out}");
    }

    #[test]
    fn check_halign_maps_all_four_alignments() {
        assert_eq!(
            TextileMlizer::check_halign(&ResolvedStyle {
                css_text: "text-align: left".to_string(),
                ..ResolvedStyle::default()
            }),
            "<"
        );
        assert_eq!(
            TextileMlizer::check_halign(&ResolvedStyle {
                css_text: "text-align: center".to_string(),
                ..ResolvedStyle::default()
            }),
            "="
        );
        assert_eq!(
            TextileMlizer::check_halign(&ResolvedStyle {
                css_text: "text-align: right".to_string(),
                ..ResolvedStyle::default()
            }),
            ">"
        );
        assert_eq!(
            TextileMlizer::check_halign(&ResolvedStyle {
                css_text: "text-align: justify".to_string(),
                ..ResolvedStyle::default()
            }),
            "<>"
        );
    }
}
