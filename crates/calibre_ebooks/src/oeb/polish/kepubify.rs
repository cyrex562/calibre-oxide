//! Port of `oeb/polish/kepubify.py`'s CSS processing (issue #652, one
//! slice of the #543 epic -- see [`super`]'s module docs; the intricate
//! sentence-span-wrapping core is #651, HTML markup/cover handling is
//! #653, the `kepubify_container`/`unkepubify_container` orchestrator is
//! #654).
//!
//! Kobo's own EPUB renderer chokes on `@page` rules and on the
//! `widows`/`orphans` properties, so kepubifying a book hides both; a
//! later un-kepubify pass needs to restore them byte-for-byte. Real
//! upstream does this with `css_parser`'s object model, which supports
//! full-fidelity CSS **comment** nodes as first-class, reinsertable
//! members of a stylesheet's rule list: a removed `@page` rule gets
//! reserialized as `/* {CSS_COMMENT_COOKIE}: <original text> */` and
//! spliced back in at the same index, safely inert to any real CSS
//! engine (which ignores comments) and cheaply reversible later.
//!
//! [`crate::css::model`] (issue #164/#590) has no such comment-as-rule
//! concept -- `cssparser`, the tokenizer it's built on, discards raw
//! `/* */` comments during tokenization rather than surfacing them as
//! AST nodes, and [`crate::css::model::Rule`] has no `Comment` variant.
//! This is a real, disclosed narrowing: instead of a comment, a hidden
//! `@page` rule is reserialized (whitespace-flattened, so it survives as
//! a single CSS string token) into a marker unknown-at-rule's string
//! prelude -- `@calibre-removed-css-for-kobo-page "<escaped text>";` --
//! which [`crate::css::parser`] already parses generically via
//! [`Rule::Unknown`] (confirmed: any unrecognized `@word ...;` round-
//! trips through `UnknownAtRule` losslessly, the same mechanism
//! `@supports`/`@keyframes` rely on elsewhere in this crate). Any real
//! CSS engine ignores an at-rule it doesn't recognize exactly as it
//! would ignore a comment, so the observable behavior (Kobo's renderer
//! never sees the hidden `@page` rule) is identical; only the concrete
//! on-disk encoding differs from real calibre's own kepub output.

use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};

use crate::covers::{generate_cover, CoverPrefs};
use crate::css::model::{Rule, Stylesheet, UnknownAtRule};
use crate::dom::{Dom, NodeId, NodeKind};
use crate::metadata::authors::authors_to_string;
use crate::metadata::meta::MetaInformation;
use crate::oeb::constants::{OEB_DOCS, OEB_STYLES};
use crate::oeb::parse_utils::merge_multiple_html_heads_and_bodies;
use crate::oeb::polish::container::{
    get_container, AnyContainer, Container, EpubContainer, ParsedItem,
};
use crate::oeb::polish::cover::{find_cover_image, find_cover_image3, find_cover_page};
use crate::oeb::polish::errors::PolishError;
use crate::oeb::polish::parsing;
use crate::oeb::polish::utils::insert_self_closing;
use crate::spell::break_iterator::sentence_positions;

/// Port of `kepubify.py`'s `CSS_COMMENT_COOKIE`.
pub const CSS_COMMENT_COOKIE: &str = "calibre-removed-css-for-kobo";

pub const KOBO_CSS_ID: &str = "kobostylehacks";
pub const EXTRA_CSS_ID: &str = "kepubify-extra-css";
pub const EXTRA_KOBO_CSS_IDS: &[&str] = &["koboSpanStyle"];
pub const KOBO_JS_NAME: &str = "kobo.js";
pub const KOBO_CSS_NAME: &str = "kobo.css";
pub const OUTER_DIV_ID: &str = "book-columns";
pub const INNER_DIV_ID: &str = "book-inner";
/// Used by [`add_kobo_spans`].
pub const KOBO_SPAN_CLASS: &str = "koboSpan";
pub const DUMMY_TITLE_PAGE_NAME: &str = "kobo-title-page-generated-by-calibre";
pub const DUMMY_COVER_IMAGE_NAME: &str = "kobo-cover-image-generated-by-calibre";
pub const KOBO_CSS: &str = "div#book-inner { margin-top: 0; margin-bottom: 0; }";

fn hidden_page_at_keyword() -> String {
    format!("{CSS_COMMENT_COOKIE}-page")
}

fn hidden_property_name(prop: &str) -> String {
    format!("-{CSS_COMMENT_COOKIE}-{prop}")
}

/// Port of `nest_css_comments()`: escapes `*/` inside text that will be
/// embedded in a real CSS comment elsewhere in this crate, so it can't
/// be closed early. Kept even though this module's own hidden-rule
/// encoding uses a quoted string, not a comment (see the module docs),
/// since #653 embeds user-supplied CSS text inside a real `<style>`
/// comment for a different reason and can reuse this helper verbatim.
pub fn nest_css_comments(text: &str) -> String {
    text.replace("*/", "*\u{200c}/")
}

/// Port of `kepubify.py`'s `Options` NamedTuple. Only
/// [`process_stylesheet`] is implemented in this module (#652); the
/// other fields exist here so #653/#654 can share one `Options` type
/// rather than each inventing their own subset.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    pub extra_css: String,
    pub hyphenation_css: String,
    pub remove_widows_and_orphans: bool,
    pub remove_at_page_rules: bool,
    /// Affects HTML markup generation (#653), not [`process_stylesheet`]
    /// -- real upstream's own `process_stylesheet` never reads this
    /// field either, confirmed directly against the Python source.
    pub prefer_justification: bool,
    pub for_removal: bool,
}

impl Options {
    /// Port of the `needs_stylesheet_processing` property.
    pub fn needs_stylesheet_processing(&self) -> bool {
        self.remove_at_page_rules || self.remove_widows_and_orphans || self.for_removal
    }
}

/// Port of `check_if_css_needs_modification()`. Real upstream's own type
/// hint says `-> tuple[bool, bool]`, but the function body actually
/// returns a 3-tuple `(sheet, remove_widows_and_orphans,
/// remove_at_page_rules)` -- a genuine stale/wrong annotation in the
/// Python source (confirmed by reading the body, not just the
/// signature). `make_options` (the only real caller) immediately
/// discards the parsed `sheet`, so this port returns just the two
/// booleans callers actually use rather than reproducing the misleading
/// annotation's shape or the unused first element.
pub fn check_if_css_needs_modification(extra_css: &str) -> (bool, bool) {
    let mut remove_widows_and_orphans = false;
    let mut remove_at_page_rules = false;
    if !extra_css.is_empty() {
        let sheet = Stylesheet::parse(extra_css);
        for rule in &sheet.rules {
            match rule {
                Rule::Page(_) => remove_at_page_rules = true,
                Rule::Style(s) => {
                    if !s.style.get_property_value("widows").is_empty()
                        || !s.style.get_property_value("orphans").is_empty()
                    {
                        remove_widows_and_orphans = true;
                    }
                }
                _ => {}
            }
            if remove_widows_and_orphans && remove_at_page_rules {
                break;
            }
        }
    }
    (remove_widows_and_orphans, remove_at_page_rules)
}

/// Arguments to [`make_options`], port of `make_options()`'s keyword
/// parameters. `remove_widows_and_orphans`/`remove_at_page_rules` are
/// `Option<bool>` to mirror Python's `bool | None = None` defaults.
#[derive(Debug, Clone)]
pub struct MakeOptionsArgs {
    pub extra_css: String,
    pub affect_hyphenation: bool,
    pub disable_hyphenation: bool,
    pub hyphenation_min_chars: u32,
    pub hyphenation_min_chars_before: u32,
    pub hyphenation_min_chars_after: u32,
    pub hyphenation_limit_lines: u32,
    pub prefer_justification: bool,
    pub remove_widows_and_orphans: Option<bool>,
    pub remove_at_page_rules: Option<bool>,
}

impl Default for MakeOptionsArgs {
    fn default() -> Self {
        Self {
            extra_css: String::new(),
            affect_hyphenation: false,
            disable_hyphenation: false,
            hyphenation_min_chars: 6,
            hyphenation_min_chars_before: 3,
            hyphenation_min_chars_after: 3,
            hyphenation_limit_lines: 2,
            prefer_justification: false,
            remove_widows_and_orphans: None,
            remove_at_page_rules: None,
        }
    }
}

/// Port of `make_options()`.
pub fn make_options(args: MakeOptionsArgs) -> Options {
    // Real upstream quirk, preserved exactly: if EITHER flag is `None`,
    // BOTH get overwritten by `check_if_css_needs_modification`'s
    // result -- even one the caller explicitly passed in gets
    // discarded, not just the missing one.
    let (remove_widows_and_orphans, remove_at_page_rules) =
        match (args.remove_widows_and_orphans, args.remove_at_page_rules) {
            (Some(w), Some(p)) => (w, p),
            _ => check_if_css_needs_modification(&args.extra_css),
        };

    let hyphenation_css = if args.affect_hyphenation {
        if args.disable_hyphenation {
            "\n* {\n  -webkit-hyphens: none !important;\n  hyphens: none !important;\n}\n".to_string()
        } else if args.hyphenation_min_chars > 0 {
            format!(
                "\n* {{\n    \
                -webkit-hyphens: auto;\n    \
                -webkit-hyphenate-limit-after: {after};\n    \
                -webkit-hyphenate-limit-before: {before};\n    \
                -webkit-hyphenate-limit-chars: {chars} {before} {after};\n    \
                -webkit-hyphenate-limit-lines: {lines};\n\n    \
                hyphens: auto;\n    \
                hyphenate-limit-chars: {chars} {before} {after};\n    \
                hyphenate-limit-lines: {lines};\n    \
                hyphenate-limit-last: page;\n}}\n\n\
                h1, h2, h3, h4, h5, h6, td {{\n    \
                -webkit-hyphens: none !important;\n    \
                hyphens: none !important;\n}}\n",
                chars = args.hyphenation_min_chars,
                before = args.hyphenation_min_chars_before,
                after = args.hyphenation_min_chars_after,
                lines = args.hyphenation_limit_lines,
            )
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    Options {
        extra_css: args.extra_css,
        hyphenation_css,
        remove_widows_and_orphans,
        remove_at_page_rules,
        prefer_justification: args.prefer_justification,
        for_removal: false,
    }
}

fn escape_hidden_payload(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

fn unescape_hidden_payload(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Hides a `@page` rule behind the marker unknown-at-rule described in
/// this module's docs. `rule` must be [`Rule::Page`].
fn hide_page_rule(rule: &Rule) -> Rule {
    let text = rule.to_css_text().replace('\n', " ");
    let escaped = escape_hidden_payload(&text);
    Rule::Unknown(UnknownAtRule {
        at_keyword: hidden_page_at_keyword(),
        prelude: format!("\"{escaped}\""),
        block: None,
    })
}

/// Reverses [`hide_page_rule`]: given the marker unknown-at-rule, parses
/// its hidden payload back into a real `@page` rule. Returns `None` if
/// `u`'s prelude isn't a well-formed hidden payload (defensive; every
/// real caller of this module only ever restores rules it hid itself).
fn restore_page_rule(u: &UnknownAtRule) -> Option<Rule> {
    let raw = u.prelude.trim();
    let inner = raw.strip_prefix('"')?.strip_suffix('"')?;
    let text = unescape_hidden_payload(inner);
    Stylesheet::parse(&text)
        .rules
        .into_iter()
        .find(|r| matches!(r, Rule::Page(_)))
}

/// Port of `process_stylesheet()`: the real, reversible CSS transform
/// kepubify/unkepubify apply to every stylesheet in a book. See the
/// module docs for the one disclosed encoding difference (marker
/// unknown-at-rule instead of a CSS comment).
pub fn process_stylesheet(css: &str, opts: &Options) -> String {
    let has_comment_cookie = css.contains(CSS_COMMENT_COOKIE);
    if opts.for_removal && !has_comment_cookie {
        return css.to_string();
    }

    let mut sheet = Stylesheet::parse(css);
    let mut changed = false;
    let hidden_kw = hidden_page_at_keyword();

    if has_comment_cookie {
        for rule in sheet.rules.iter_mut() {
            if let Rule::Style(s) = rule {
                for q in ["widows", "orphans"] {
                    let hidden_name = hidden_property_name(q);
                    if let Some(d) = s
                        .style
                        .properties
                        .iter_mut()
                        .find(|d| d.name.eq_ignore_ascii_case(&hidden_name))
                    {
                        d.name = q.to_string();
                        changed = true;
                    }
                }
            }
        }
        for rule in sheet.rules.iter_mut() {
            let is_hidden_page = matches!(rule, Rule::Unknown(u) if u.at_keyword.eq_ignore_ascii_case(&hidden_kw));
            if is_hidden_page {
                if let Rule::Unknown(u) = rule {
                    if let Some(restored) = restore_page_rule(u) {
                        *rule = restored;
                        changed = true;
                    }
                }
            }
        }
    }

    if opts.for_removal {
        return if changed { sheet.to_css_text() } else { css.to_string() };
    }

    if opts.remove_widows_and_orphans {
        for rule in sheet.rules.iter_mut() {
            if let Rule::Style(s) = rule {
                for q in ["widows", "orphans"] {
                    if let Some(d) = s
                        .style
                        .properties
                        .iter_mut()
                        .find(|d| d.name.eq_ignore_ascii_case(q))
                    {
                        changed = true;
                        d.name = hidden_property_name(q);
                    }
                }
            }
        }
    }

    if opts.remove_at_page_rules {
        for rule in sheet.rules.iter_mut() {
            if matches!(rule, Rule::Page(_)) {
                changed = true;
                *rule = hide_page_rule(rule);
            }
        }
    }

    if changed {
        sheet.to_css_text()
    } else {
        css.to_string()
    }
}

// ---------------------------------------------------------------------
// HTML markup + title-page/cover handling (issue #653), except
// `add_kobo_markup_to_html`/`remove_kobo_markup_from_html` themselves --
// real upstream's own bodies call straight into `add_kobo_spans`/
// `unwrap`/`remove_kobo_spans`, the intricate sentence-span-wrapping
// core split out as issue #651 (not yet ported). Everything below is
// independent of that core and real end to end.
// ---------------------------------------------------------------------

fn is_href_to_fname(href: Option<&str>, fname: &str) -> bool {
    href.map(|h| h.rsplit('/').next().unwrap_or(h) == fname).unwrap_or(false)
}

/// Port of `add_style_and_script`: injects the Kobo stylesheet/extra-CSS/
/// script tags into `root`'s last `<head>`, or its last `<body>` if it
/// has no `<head>`. Returns whether either was found.
pub fn add_style_and_script(dom: &mut Dom, root: NodeId, kobo_js_href: &str, opts: &Options) -> bool {
    let heads: Vec<NodeId> = dom.children(root).into_iter().filter(|&c| dom.tag(c) == Some("head")).collect();
    if let Some(&head) = heads.last() {
        add_style_and_script_to(dom, head, kobo_js_href, opts);
        return true;
    }
    let bodies: Vec<NodeId> = dom.children(root).into_iter().filter(|&c| dom.tag(c) == Some("body")).collect();
    if let Some(&body) = bodies.last() {
        add_style_and_script_to(dom, body, kobo_js_href, opts);
        return true;
    }
    false
}

fn add_style_and_script_to(dom: &mut Dom, parent: NodeId, kobo_js_href: &str, opts: &Options) {
    let style = dom.new_element("style");
    dom.node_mut(style).attrs.insert("type".to_string(), "text/css".to_string());
    dom.node_mut(style).attrs.insert("id".to_string(), KOBO_CSS_ID.to_string());
    let text = dom.new_text(KOBO_CSS);
    dom.append_child(style, text);
    insert_self_closing(dom, parent, style, None);

    let extra_css = format!("{}\n\n{}", opts.hyphenation_css, opts.extra_css).trim().to_string();
    if !extra_css.is_empty() {
        let extra = dom.new_element("style");
        dom.node_mut(extra).attrs.insert("type".to_string(), "text/css".to_string());
        dom.node_mut(extra).attrs.insert("id".to_string(), EXTRA_CSS_ID.to_string());
        let extra_text = dom.new_text(&format!("\n{extra_css}"));
        dom.append_child(extra, extra_text);
        insert_self_closing(dom, parent, extra, None);
    }

    let script = dom.new_element("script");
    dom.node_mut(script).attrs.insert("type".to_string(), "text/javascript".to_string());
    dom.node_mut(script).attrs.insert("src".to_string(), kobo_js_href.to_string());
    insert_self_closing(dom, parent, script, None);
}

/// Port of `remove_kobo_styles_and_scripts`.
pub fn remove_kobo_styles_and_scripts(dom: &mut Dom, root: NodeId) {
    let mut ids_to_remove: Vec<&str> = EXTRA_KOBO_CSS_IDS.to_vec();
    ids_to_remove.push(KOBO_CSS_ID);
    ids_to_remove.push(EXTRA_CSS_ID);

    for style in dom.find_all_tag(root, "style") {
        if let Some(id) = dom.node(style).attrs.get("id") {
            if ids_to_remove.contains(&id.as_str()) {
                dom.detach(style);
            }
        }
    }
    for link in dom.find_all_tag(root, "link") {
        let attrs = &dom.node(link).attrs;
        let rel = attrs.get("rel").map(String::as_str);
        let typ = attrs.get("type").map(String::as_str);
        let href = attrs.get("href").map(String::as_str);
        if rel == Some("stylesheet") && typ == Some("text/css") && is_href_to_fname(href, KOBO_CSS_NAME) {
            dom.detach(link);
        }
    }
    for script in dom.find_all_tag(root, "script") {
        let attrs = &dom.node(script).attrs;
        let typ = attrs.get("type").map(String::as_str);
        let src = attrs.get("src").map(String::as_str);
        if typ == Some("text/javascript") && is_href_to_fname(src, KOBO_JS_NAME) {
            dom.detach(script);
        }
    }
    let mut comments = Vec::new();
    find_all_comments(dom, root, &mut comments);
    for c in comments {
        if let NodeKind::Comment(text) = &dom.node(c).kind {
            if text == " kobo-style " {
                dom.detach(c);
            }
        }
    }
}

fn find_all_comments(dom: &Dom, id: NodeId, out: &mut Vec<NodeId>) {
    if matches!(dom.node(id).kind, NodeKind::Comment(_)) {
        out.push(id);
    }
    for c in dom.node(id).children.clone() {
        find_all_comments(dom, c, out);
    }
}

/// Port of `wrap_body_contents`: wraps `body`'s contents in the
/// `div#book-columns > div#book-inner` pair Kobo's renderer expects for
/// pagination styles, returning the inner div. Real upstream's own
/// pre-cleanup loop (stripping any pre-existing `id="book-columns"`/
/// `id="book-inner"` from arbitrary elements before wrapping, to avoid
/// ID collisions) has a genuine bug: its `INNER_DIV_ID` half is built as
/// `@id={INNER_DIV_ID}` -- missing the quotes the `OUTER_DIV_ID` half
/// has -- so the substituted XPath text is `@id=book-inner`. Since `-`
/// is a legal XPath NCName character, that lexes as `@id =
/// child::book-inner` (a node-set test), not a string comparison; it
/// silently matches nothing, since no literal `<book-inner>` element
/// ever exists. Only the `OUTER_DIV_ID` cleanup (correctly quoted) ever
/// actually runs in real calibre. Replicated bug-for-bug -- this port
/// targets real observed behavior, not the obviously-intended one.
pub fn wrap_body_contents(dom: &mut Dom, body: NodeId) -> NodeId {
    let doc_root = dom.root;
    for elem in dom.preorder_elements(doc_root) {
        if dom.node(elem).attrs.get("id").map(String::as_str) == Some(OUTER_DIV_ID) {
            dom.node_mut(elem).attrs.shift_remove("id");
        }
    }
    let outer = dom.new_element("div");
    dom.node_mut(outer).attrs.insert("id".to_string(), OUTER_DIV_ID.to_string());
    let inner = dom.new_element("div");
    dom.node_mut(inner).attrs.insert("id".to_string(), INNER_DIV_ID.to_string());
    dom.append_child(outer, inner);

    for child in dom.children(body) {
        dom.append_child(inner, child);
    }
    dom.append_child(body, outer);
    inner
}

/// Port of `unwrap_body_contents`: the reverse of [`wrap_body_contents`].
pub fn unwrap_body_contents(dom: &mut Dom, body: NodeId) {
    let mut inners = Vec::new();
    for outer in dom.children(body) {
        if dom.tag(outer) != Some("div") || dom.node(outer).attrs.get("id").map(String::as_str) != Some(OUTER_DIV_ID) {
            continue;
        }
        for inner in dom.children(outer) {
            if dom.tag(inner) == Some("div") && dom.node(inner).attrs.get("id").map(String::as_str) == Some(INNER_DIV_ID) {
                inners.push(inner);
            }
        }
    }

    let mut new_children = Vec::new();
    let mut outers_to_remove = Vec::new();
    for inner in &inners {
        new_children.extend(dom.children(*inner));
        if let Some(p) = dom.parent(*inner) {
            outers_to_remove.push(p);
        }
    }
    for outer in outers_to_remove {
        dom.detach(outer);
    }
    for child in new_children {
        dom.append_child(body, child);
    }
}

fn clear_leading_text(dom: &mut Dom, id: NodeId) {
    let children = dom.children(id);
    let mut to_remove = Vec::new();
    for c in children {
        match dom.node(c).kind {
            NodeKind::Text(_) => to_remove.push(c),
            NodeKind::Element(_) => break,
            _ => {}
        }
    }
    for c in to_remove {
        dom.detach(c);
    }
}

/// Port of `is_probably_a_title_page`. Assumes a `<title>` element never
/// contains nested markup (always true for real HTML), so `<title>`'s
/// full recursive text content stands in for lxml's `.text` (the text
/// immediately following the opening tag). Real upstream mutates the
/// tree as a side effect: every `<title>` up to (excluding) the first
/// one whose words contain "cover" has its text cleared, replicated
/// here via [`clear_leading_text`], since the whole point is excluding
/// title text from the page's own visible-text length below.
pub fn is_probably_a_title_page(dom: &mut Dom, root: NodeId) -> bool {
    for title in dom.find_all_tag(root, "title") {
        let text = dom.text_content(title);
        if !text.is_empty() {
            let is_cover = text.to_lowercase().split_whitespace().any(|w| w == "cover");
            if is_cover {
                return true;
            }
        }
        clear_leading_text(dom, title);
    }
    let text = dom.text_content(root);
    let textlen = text.chars().filter(|c| !c.is_whitespace()).count();
    let num_images = dom.find_all_tag(root, "img").len();
    let num_svgs = dom.find_all_tag(root, "svg").len();
    (num_images + num_svgs == 1 && textlen <= 10) || (textlen <= 50 && (num_images + num_svgs) < 1)
}

/// Port of `add_dummy_title_page`. `mi_title`/`mi_authors` are `mi`'s
/// two fields upstream's template actually reads. Real upstream's own
/// template has a second, unrelated bug worth noting rather than
/// "fixing": the author line is `<h3 ...>{aus}</h1>` -- a mismatched
/// closing tag -- preserved verbatim so [`Dom::parse`] (html5ever, the
/// same class of tolerant HTML5 parser real calibre's own `html5-parser`
/// backend is) recovers from it exactly as any real browser would,
/// rather than silently correcting upstream's own markup.
///
/// One deliberate departure from upstream's literal template text: the
/// `<script .../>` tag is written with an explicit `</script>` here
/// instead of a self-closing slash. `oeb::polish::container`'s own
/// `parse_xhtml` "always takes the tag-soup path" (its own module
/// docs), and HTML5 tag-soup parsing (unlike the strict XML parsing
/// real calibre applies to `.xhtml` container members) does not treat
/// a trailing `/>` on a non-void element like `<script>` as
/// self-closing -- it opens the tag and reads everything up to the
/// next literal `</script>` as raw script text, silently swallowing
/// the rest of the document. This is a pre-existing, already-documented
/// property of this crate's `Dom`, not something new to this function;
/// avoiding self-closing syntax on non-void elements sidesteps it.
pub fn add_dummy_title_page(
    container: &mut Container,
    cover_image_name: Option<&str>,
    mi_title: &str,
    mi_authors: &[String],
    kobo_js_name: &str,
) -> Result<String> {
    let titlepage_name = container.add_file(
        &format!("{DUMMY_TITLE_PAGE_NAME}.xhtml"),
        b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><head></head><body></body></html>",
        None,
        Some(0),
        true,
    )?;
    let kobo_js_href = container.name_to_href(kobo_js_name, Some(&titlepage_name));
    let content = if let Some(cover_image_name) = cover_image_name {
        let cover_href = container.name_to_href(cover_image_name, Some(&titlepage_name));
        format!(r#"<img src="{cover_href}" alt="cover" style="height: 100%" />"#)
    } else {
        let aus = authors_to_string(mi_authors);
        format!(
            "\n        <h1 style=\"text-align: center\">{mi_title}</h1>\n        <h3 style=\"text-align: center\">{aus}</h1>\n        "
        )
    };
    let html = format!(
        "<?xml version='1.0' encoding='utf-8'?>\n\
<html xmlns=\"http://www.w3.org/1999/xhtml\" lang=\"en\" xml:lang=\"en\">\n\
    <head>\n\
        <meta http-equiv=\"Content-Type\" content=\"text/html; charset=utf-8\"/>\n\
        <title>Dummy title page created by calibre</title>\n\
        <style type=\"text/css\">\n\
            @page {{ padding: 0pt; margin:0pt }}\n\
            body {{ text-align: center; padding:0pt; margin: 0pt }}\n\
            div {{ padding:0pt; margin: 0pt }}\n\
            img {{ padding:0pt; margin: 0pt }}\n\
        </style>\n\
        <style type=\"text/css\" id=\"{KOBO_CSS_ID}\">\n\
        {KOBO_CSS}\n\
        </style>\n\
        <script type=\"text/javascript\" src=\"{kobo_js_href}\"></script>\n\
    </head>\n\
    <body><div id=\"{OUTER_DIV_ID}\"><div id=\"{INNER_DIV_ID}\">\n\
    {content}\n\
    </div></div></body>\n\
</html>\n"
    );
    let dom = Dom::parse(&html);
    container.base.parsed_cache.insert(titlepage_name.clone(), ParsedItem::Xhtml(dom));
    container.dirty(&titlepage_name);
    container.apply_unique_properties(Some(&titlepage_name), &["calibre:title-page"])?;
    Ok(titlepage_name)
}

/// Port of `remove_dummy_title_page`.
pub fn remove_dummy_title_page(container: &mut Container) -> Result<()> {
    for (name, is_linear) in container.spine_names()? {
        if is_linear {
            if name.contains(DUMMY_TITLE_PAGE_NAME) {
                container.remove_item(&name, true)?;
            }
            break;
        }
    }
    Ok(())
}

/// Port of `remove_dummy_cover_image`.
pub fn remove_dummy_cover_image(container: &mut Container) -> Result<()> {
    let names: Vec<String> = container.base.mime_map.keys().cloned().collect();
    for name in names {
        if name.contains(DUMMY_COVER_IMAGE_NAME) {
            container.remove_item(&name, true)?;
        }
    }
    Ok(())
}

/// Port of `first_spine_item_is_probably_title_page`.
pub fn first_spine_item_is_probably_title_page(container: &mut Container) -> Result<bool> {
    for (name, is_linear) in container.spine_names()? {
        if !is_linear {
            continue;
        }
        let fname = name.rsplit('/').next().unwrap_or(&name);
        if name.contains("cover") || fname.contains("title") {
            return Ok(true);
        }
        container.ensure_parsed(&name)?;
        let root = container.get_xhtml(&name)?.root;
        let dom = container.get_xhtml_mut(&name)?;
        return Ok(is_probably_a_title_page(dom, root));
    }
    Ok(false)
}

/// Convenience wrapper matching real `mi`'s field shape, so callers
/// (issue #654's orchestrator) don't need to destructure
/// [`MetaInformation`] themselves.
pub fn add_dummy_title_page_for(
    container: &mut Container,
    cover_image_name: Option<&str>,
    mi: &MetaInformation,
    kobo_js_name: &str,
) -> Result<String> {
    add_dummy_title_page(container, cover_image_name, &mi.title, &mi.authors, kobo_js_name)
}

// ---------------------------------------------------------------------
// Kobo sentence/paragraph span wrapping (issue #651)
// ---------------------------------------------------------------------
//
// This is the real core of kepubification: Kobo's renderer can't do
// highlighting or bookmarking without every text run being wrapped in
// its own identified `<span>`, so `add_kobo_spans` walks the body and
// wraps each sentence in `<span class="koboSpan" id="kobo.PARA.SEG">`.
//
// # How upstream's lxml-shaped algorithm maps onto [`Dom`]
//
// Real upstream is written against lxml, where text is *not* a node:
// each element carries a `.text` (the run before its first child) and a
// `.tail` (the run after its own closing tag). Nearly all of
// `wrap_text_in_spans`' apparent complexity is bookkeeping for that
// split representation:
//
// * It must compute an insertion index `at` (`0` for `.text`, else
//   `parent.index(after_child) + 1`), then write the surviving leading
//   whitespace to *either* `parent.text` *or* `parent[at-1].tail`
//   depending on which case it's in, then `insert()` each new span at
//   `at`, `at+1`, ...
// * It needs a `try/except ValueError` fallback, because by the time a
//   run's `.tail` is processed its `after_child` may have been moved
//   inside a wrapper span by `wrap_child`, so `parent.index()` on it
//   raises.
//
// In this crate's [`Dom`] a text run *is* an ordinary child node, and
// every one of those branches collapses to the same statement: **replace
// the text node, in place, with `[surviving-whitespace?, span, span,
// ...]`**. The `ValueError` fallback disappears entirely (a wrapped
// child's following text node keeps its own position, so nothing needs
// re-deriving), and `at == 0` is just "this text node is `children[0]`".
// The three-way `parent.text` / `parent[at-1].tail` / `insert(at, ...)`
// split is one splice. This is a representation difference, not a
// behavioral narrowing -- the resulting trees are identical.
//
// The same applies to the stack walk. Upstream pushes each child's
// `.tail` and the child itself, then handles `node.text` *inline*
// (before any pop) because `.text` isn't a child it can push. Here,
// pushing every child -- text and element alike -- in reverse order
// gives the identical pop order, because a leading text run is simply
// `children[0]` and so is pushed last and popped first.

/// Port of `SKIPPED_TAGS`. Upstream's set also contains `''`, which is
/// what its `barename(child.tag) if isinstance(child.tag, str) else ''`
/// yields for a comment or processing instruction -- i.e. `''` exists
/// purely to stop the walk descending into those. Here they simply
/// aren't [`NodeKind::Element`]s and are skipped structurally, so the
/// sentinel has nothing to do and is dropped.
pub const SKIPPED_TAGS: &[&str] = &["script", "style", "atom", "pre", "audio", "video", "svg", "math"];

/// Port of `BLOCK_TAGS`: the tags that start a new Kobo "paragraph"
/// (the first number in a `kobo.PARA.SEG` id).
pub const BLOCK_TAGS: &[&str] = &["p", "ol", "ul", "table", "h1", "h2", "h3", "h4", "h5", "h6"];

/// Port of `tts.py`'s `lang_for_elem` -- the one helper `kepubify.py`
/// borrows from that module (see this crate's issue #543 notes: the
/// "kepubify is blocked on tts.py" framing turned out to rest entirely
/// on this two-line function).
///
/// Upstream tries `lang`, then `xml_lang`, then the Clark-notation
/// `{http://www.w3.org/XML/1998/namespace}lang`; the middle spelling is
/// what calibre's own OEB parser rewrites `xml:lang` to. [`Dom::parse`]
/// keys attributes by local name, so the two XML spellings collapse to
/// the one `"xml:lang"` key -- the same pair `oeb::polish::hyphenation`
/// and `oeb::polish::spell` already check, followed here for
/// consistency. An empty value falls through to the next spelling,
/// matching Python's `or` chain over attribute *values*.
pub fn lang_for_elem(dom: &Dom, elem: NodeId, parent_lang: &str) -> String {
    let attrs = &dom.node(elem).attrs;
    let raw = ["lang", "xml:lang"]
        .iter()
        .find_map(|k| attrs.get(*k).map(String::as_str).filter(|v| !v.is_empty()));
    raw.and_then(calibre_utils::localization::canonicalize_lang)
        .unwrap_or_else(|| parent_lang.to_string())
}

/// One entry of `add_kobo_spans`' explicit walk stack.
enum SpanTask {
    /// An element to descend into. Upstream's `(elem, None, tagname, lang)`.
    Element { node: NodeId, tag: String, lang: String },
    /// A text run to wrap. Upstream's `(text, parent, after_child, lang)`
    /// -- here the run's own node id stands in for both the text and the
    /// `after_child` position marker (see this section's module notes).
    Text { node: NodeId, parent: NodeId, lang: String },
}

/// The `nonlocal` state of upstream's nested closures.
struct KoboSpanWrapper<'a> {
    dom: &'a mut Dom,
    paranum: u32,
    segnum: u32,
    increment_next_para: bool,
    prefer_justification: bool,
}

/// Port of lxml's `len(element)`: the number of children that are not
/// text. Comments count, exactly as they do in lxml.
fn non_text_child_count(dom: &Dom, parent: NodeId) -> usize {
    dom.node(parent)
        .children
        .iter()
        .filter(|&&c| !matches!(dom.node(c).kind, NodeKind::Text(_)))
        .count()
}

fn set_text(dom: &mut Dom, node: NodeId, value: &str) {
    if let NodeKind::Text(t) = &mut dom.node_mut(node).kind {
        value.clone_into(t);
    }
}

impl KoboSpanWrapper<'_> {
    /// Port of the `kobo_span` closure. Note the increment happens
    /// *before* the id is formatted, so the first span of a paragraph is
    /// `kobo.N.1`, not `kobo.N.0`.
    fn kobo_span(&mut self) -> NodeId {
        self.segnum += 1;
        let s = self.dom.new_element("span");
        let attrs = &mut self.dom.node_mut(s).attrs;
        attrs.insert("class".to_string(), KOBO_SPAN_CLASS.to_string());
        attrs.insert("id".to_string(), format!("kobo.{}.{}", self.paranum, self.segnum));
        s
    }

    /// Port of the `wrap_child` closure: replaces `child` (only ever an
    /// `<img>`) with a span wrapping it.
    ///
    /// Upstream additionally does `w.tail = child.tail` and
    /// `child.tail = child.text = None`, all three of which are
    /// structurally automatic here: the run following `child` is its own
    /// sibling node and simply ends up following the wrapper instead,
    /// and an `<img>` is a void element, so it has no children to clear.
    fn wrap_child(&mut self, child: NodeId) {
        self.increment_next_para = false;
        self.paranum += 1;
        self.segnum = 0;
        let (Some(parent), Some(idx)) = (self.dom.parent(child), self.dom.index_in_parent(child)) else {
            return;
        };
        let w = self.kobo_span();
        self.dom.insert_child(parent, idx, w);
        self.dom.append_child(w, child);
    }

    /// Port of the `wrap_text_in_spans` closure.
    fn wrap_text_in_spans(&mut self, text_node: NodeId, parent: NodeId, lang: &str) {
        let NodeKind::Text(text) = self.dom.node(text_node).kind.clone() else {
            return;
        };
        let stripped = text.trim_start_matches(char::is_whitespace).to_string();
        // Upstream's `at`: `0` exactly when this run is `parent.text`,
        // which here means it is `parent`'s first child.
        let is_leading = self.dom.index_in_parent(text_node) == Some(0);

        if self.increment_next_para {
            self.paranum += 1;
            self.segnum = 0;
            self.increment_next_para = false;
        }

        // "block tag with only whitespace": the whole run, whitespace
        // and all, goes inside one span. Reached before the pure-
        // whitespace early return below, so such a block still gets a
        // span (and consumed a paragraph number just above).
        if is_leading && stripped.is_empty() && non_text_child_count(self.dom, parent) == 0 {
            let s = self.kobo_span();
            let t = self.dom.new_text(&text);
            self.dom.append_child(s, t);
            self.dom.detach(text_node);
            self.dom.append_child(parent, s);
            return;
        }

        let leading_ws_len = text.len() - stripped.len();
        let leading_whitespace = (leading_ws_len > 0).then(|| text[..leading_ws_len].to_string());
        // When there is real text and we aren't justifying, the leading
        // whitespace is pulled *into* the first span rather than left
        // outside it -- that is what keeps Kobo's own highlighting from
        // leaving an unhighlighted gap at the start of a sentence.
        let before = if !stripped.is_empty() && !self.prefer_justification {
            None
        } else {
            leading_whitespace.clone()
        };

        let idx = self.dom.index_in_parent(text_node).expect("text run is attached to its parent");
        let mut at = match &before {
            Some(b) => {
                set_text(self.dom, text_node, b);
                idx + 1
            }
            None => {
                self.dom.detach(text_node);
                idx
            }
        };

        // Pure whitespace between elements: left exactly as it was.
        if stripped.is_empty() {
            return;
        }

        let body = if leading_whitespace.is_none() || self.prefer_justification {
            stripped.as_str()
        } else {
            text.as_str()
        };

        for (pos, sz) in sentence_positions(body, lang) {
            let inside = &body[pos..pos + sz];
            // With `prefer_justification`, a sentence's trailing
            // whitespace is moved out of the span, so Kobo's justifier
            // sees a normal inter-span gap instead of trailing space
            // locked inside an inline box.
            let (span_text, tail) = if self.prefer_justification {
                let trimmed = inside.trim_end_matches(char::is_whitespace);
                if trimmed.len() == inside.len() {
                    (inside, None)
                } else {
                    (trimmed, Some(&inside[trimmed.len()..]))
                }
            } else {
                (inside, None)
            };

            let s = self.kobo_span();
            let t = self.dom.new_text(span_text);
            self.dom.append_child(s, t);
            self.dom.insert_child(parent, at, s);
            at += 1;
            if let Some(tail) = tail {
                let tn = self.dom.new_text(tail);
                self.dom.insert_child(parent, at, tn);
                at += 1;
            }
        }
    }
}

/// Port of `add_kobo_spans(inner, root_lang, prefer_justification)`.
///
/// `inner` is the `div#book-inner` [`wrap_body_contents`] just created.
/// Every text run beneath it is split at sentence boundaries (via the
/// already-real [`sentence_positions`]) and each sentence wrapped in
/// `<span class="koboSpan" id="kobo.PARA.SEG">`; every `<img>` is
/// wrapped in a span of its own.
///
/// `lang` reaches [`sentence_positions`], whose own port segments text
/// with `unicode-segmentation` rather than ICU and so ignores it (see
/// `crate::spell::break_iterator`); the full language-inheritance walk
/// is still ported faithfully here so that behavior arrives for free if
/// that module ever grows real per-language iterators.
pub fn add_kobo_spans(dom: &mut Dom, inner: NodeId, root_lang: &str, prefer_justification: bool) {
    let root_tag = dom.tag(inner).unwrap_or("").to_ascii_lowercase();
    let root_lang = lang_for_elem(dom, inner, root_lang);
    let mut w = KoboSpanWrapper {
        dom,
        paranum: 0,
        segnum: 0,
        increment_next_para: true,
        prefer_justification,
    };
    let mut stack = vec![SpanTask::Element {
        node: inner,
        tag: root_tag,
        lang: root_lang,
    }];

    while let Some(task) = stack.pop() {
        let (node, tag, lang) = match task {
            SpanTask::Text { node, parent, lang } => {
                w.wrap_text_in_spans(node, parent, &lang);
                continue;
            }
            SpanTask::Element { node, tag, lang } => (node, tag, lang),
        };

        if tag == "img" {
            w.wrap_child(node);
            continue;
        }
        if !w.increment_next_para && BLOCK_TAGS.contains(&tag.as_str()) {
            w.increment_next_para = true;
        }

        // Reverse order so the pops come back out in document order.
        // Unlike upstream this also covers the leading run, which here
        // is just `children[0]` and so is pushed last and popped first
        // -- exactly where upstream's inline `if node.text:` handled it.
        for child in w.dom.node(node).children.clone().into_iter().rev() {
            let is_text = matches!(&w.dom.node(child).kind, NodeKind::Text(t) if !t.is_empty());
            if is_text {
                stack.push(SpanTask::Text {
                    node: child,
                    parent: node,
                    lang: lang.clone(),
                });
                continue;
            }
            let Some(child_tag) = w.dom.tag(child).map(|t| t.to_ascii_lowercase()) else {
                continue;
            };
            if SKIPPED_TAGS.contains(&child_tag.as_str()) {
                continue;
            }
            let child_lang = lang_for_elem(w.dom, child, &lang);
            stack.push(SpanTask::Element {
                node: child,
                tag: child_tag,
                lang: child_lang,
            });
        }
    }
}

/// Port of `unwrap(span)`: removes one Kobo span, splicing whatever it
/// held back into its parent.
///
/// Two real upstream quirks are reproduced rather than quietly repaired,
/// because both are unreachable for any book this crate itself produces
/// and "fix" would mean guessing at an intent upstream never stated:
///
/// * When the span has element children (only ever the `<img>` case),
///   upstream's `del p[idx]` drops the span *and its tail* -- in lxml a
///   tail belongs to its element -- and then reinserts only `span[0]`.
///   So both the text after the span and any second element child are
///   discarded. After a real `add_kobo_spans` run an image wrapper never
///   has either (the following run was consumed into its own spans, and
///   `wrap_child` only ever puts one child inside), so this is
///   unreachable in practice.
/// * When the span is empty, upstream's `span.text + (span.tail or '')`
///   raises `TypeError`, since `span.text` is then `None`. Every span
///   `kobo_span` builds has text, so this too is unreachable; an empty
///   span is treated as empty text here rather than panicking.
pub fn unwrap(dom: &mut Dom, span: NodeId) {
    let (Some(parent), Some(idx)) = (dom.parent(span), dom.index_in_parent(span)) else {
        return;
    };
    let first_element_child = dom
        .node(span)
        .children
        .iter()
        .copied()
        .find(|&c| !matches!(dom.node(c).kind, NodeKind::Text(_)));
    // lxml's `span.tail`: the run immediately following the span.
    let tail_node = dom
        .node(parent)
        .children
        .get(idx + 1)
        .copied()
        .filter(|&n| matches!(dom.node(n).kind, NodeKind::Text(_)));

    if let Some(child) = first_element_child {
        if let Some(tail) = tail_node {
            dom.detach(tail);
        }
        dom.detach(span);
        dom.insert_child(parent, idx, child);
        return;
    }

    let mut text = dom.text_content(span);
    if let Some(tail) = tail_node {
        text.push_str(&dom.text_content(tail));
        dom.detach(tail);
    }
    dom.detach(span);

    // Merge into the preceding run if there is one, keeping lxml's
    // invariant that two text runs are never adjacent. Upstream phrases
    // this as "append to `p[idx-1]`'s tail, else to `p.text`"; both mean
    // the same node here, and when the preceding sibling is an element
    // with no tail at all, giving it one *is* inserting a run at `idx`.
    if idx > 0 {
        let prev = dom.node(parent).children[idx - 1];
        if let NodeKind::Text(t) = &mut dom.node_mut(prev).kind {
            t.push_str(&text);
            return;
        }
    }
    let t = dom.new_text(&text);
    dom.insert_child(parent, idx, t);
}

/// Port of `remove_kobo_spans(body)`. Returns whether any span was found.
pub fn remove_kobo_spans(dom: &mut Dom, body: NodeId) -> bool {
    let spans: Vec<NodeId> = dom
        .find_all_tag(body, "span")
        .into_iter()
        .filter(|&s| {
            let attrs = &dom.node(s).attrs;
            attrs.get("class").map(String::as_str) == Some(KOBO_SPAN_CLASS)
                && attrs.get("id").is_some_and(|i| i.starts_with("kobo."))
        })
        .collect();
    let found = !spans.is_empty();
    for span in spans {
        unwrap(dom, span);
    }
    found
}

/// Narrow stand-in for `calibre.utils.localization.get_lang`, matching
/// the identical one `oeb::polish::toc` already documents: no locale
/// subsystem exists in this port, so this is always the fallback
/// `get_lang` itself lands on when nothing is configured.
fn get_lang() -> &'static str {
    "eng"
}

/// Port of `add_kobo_markup_to_html`.
pub fn add_kobo_markup_to_html(
    dom: &mut Dom,
    root: NodeId,
    kobo_js_href: &str,
    opts: &Options,
    metadata_lang: &str,
) {
    let base = if metadata_lang.is_empty() { get_lang() } else { metadata_lang };
    let base = calibre_utils::localization::canonicalize_lang(base).unwrap_or_default();
    let root_lang = lang_for_elem(dom, root, &base);
    let root_lang = calibre_utils::localization::canonicalize_lang(if root_lang.is_empty() { "en" } else { &root_lang })
        .unwrap_or_default();

    add_style_and_script(dom, root, kobo_js_href, opts);

    let bodies: Vec<NodeId> = dom.children(root).into_iter().filter(|&c| dom.tag(c) == Some("body")).collect();
    for body in bodies {
        let body_lang = lang_for_elem(dom, body, &root_lang);
        let inner = wrap_body_contents(dom, body);
        add_kobo_spans(dom, inner, &body_lang, opts.prefer_justification);
    }
}

/// Port of `remove_kobo_markup_from_html`.
pub fn remove_kobo_markup_from_html(dom: &mut Dom, root: NodeId) {
    remove_kobo_styles_and_scripts(dom, root);
    let bodies: Vec<NodeId> = dom.children(root).into_iter().filter(|&c| dom.tag(c) == Some("body")).collect();
    for body in bodies {
        unwrap_body_contents(dom, body);
        remove_kobo_spans(dom, body);
    }
}

// ---------------------------------------------------------------------
// Container-wide orchestrators (issue #654)
// ---------------------------------------------------------------------
//
// Everything above operates on one parsed document at a time; this
// section ties it together the way real upstream's `kepubify_container`/
// `unkepubify_container` do: find/generate a cover, add a dummy title
// page if needed, then dispatch every manifest document/stylesheet
// through [`kepubify_parsed_html`]/[`process_stylesheet`].
//
// Real upstream dispatches per-file work across a `ThreadPoolExecutor`
// sized by `calculate_number_of_workers` (`calibre.srv.render_book`,
// itself a heuristic over free memory/file sizes). This port reuses this
// crate's own established `std::thread::scope` + shared-queue pattern
// instead (see `comic::page_processor::process_pages`,
// `web::feeds::download::build_index`) with a plain
// `available_parallelism()` worker count -- a disclosed narrowing of the
// exact sizing heuristic, not of the parallelism itself: every file
// still gets processed, just with a simpler worker-count formula.

/// Embedded copy of `resources/templates/kobo.js` (Kobo's own
/// pagination/highlighting shim), port of `kobo_js()`.
const KOBO_JS_TEMPLATE: &[u8] = include_bytes!("../../../resources/templates/kobo.js");

pub fn kobo_js() -> &'static [u8] {
    KOBO_JS_TEMPLATE
}

/// Port of `serialize_html`. [`Dom::serialize`]'s own escaping (shared
/// with every other writer in this crate) only covers `<`/`>`/`&`/
/// quotes, not non-ASCII text, so -- exactly like real upstream's own
/// post-`tostring` fixup -- a non-breaking space is turned into a
/// numeric entity afterwards as a plain string replace.
pub fn serialize_html(dom: &Dom) -> Vec<u8> {
    let ans = dom.serialize(dom.root).replace('\u{a0}', "&#160;");
    let mut out = b"<?xml version='1.0' encoding='utf-8'?>\n".to_vec();
    out.extend_from_slice(ans.as_bytes());
    out
}

fn style_text(dom: &Dom, style: NodeId) -> Option<String> {
    let &first = dom.node(style).children.first()?;
    match &dom.node(first).kind {
        NodeKind::Text(t) => Some(t.clone()),
        _ => None,
    }
}

fn set_style_text(dom: &mut Dom, style: NodeId, value: &str) {
    if let Some(&first) = dom.node(style).children.first() {
        set_text(dom, first, value);
    }
}

/// Port of `kepubify_parsed_html`. `root` is the document's `<html>`
/// element (from [`Dom::find_first_tag_global`]), not [`Dom::root`]
/// (the document pseudo-node) -- matching every other function in this
/// module that takes a `root: NodeId`.
pub fn kepubify_parsed_html(dom: &mut Dom, root: NodeId, kobo_js_href: &str, opts: &Options, metadata_lang: &str) {
    remove_kobo_markup_from_html(dom, root);
    if !opts.for_removal {
        merge_multiple_html_heads_and_bodies(dom, root);
    }
    if opts.needs_stylesheet_processing() {
        for style in dom.find_all_tag(root, "style") {
            let typ = dom.node(style).attrs.get("type").cloned().unwrap_or_else(|| "text/css".to_string());
            if typ != "text/css" {
                continue;
            }
            if let Some(text) = style_text(dom, style) {
                if !text.is_empty() {
                    let processed = process_stylesheet(&text, opts);
                    if processed != text {
                        set_style_text(dom, style, &processed);
                    }
                }
            }
        }
    }
    if !opts.for_removal {
        add_kobo_markup_to_html(dom, root, kobo_js_href, opts, metadata_lang);
    }
}

/// Port of `kepubify_html_data`.
pub fn kepubify_html_data(raw: &[u8], kobo_js_href: &str, opts: &Options, metadata_lang: &str) -> Dom {
    let (text, _encoding) = crate::chardet::xml_to_unicode(raw, false, true);
    let mut dom = parsing::parse(&text, true, false);
    if let Some(root) = dom.find_first_tag_global("html") {
        kepubify_parsed_html(&mut dom, root, kobo_js_href, opts, metadata_lang);
    }
    dom
}

/// Port of `kepubify_html_path`.
pub fn kepubify_html_path(path: &Path, kobo_js_href: &str, metadata_lang: &str, opts: &Options) -> Result<()> {
    let raw = fs::read(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let dom = kepubify_html_data(&raw, kobo_js_href, opts, metadata_lang);
    let out = serialize_html(&dom);
    fs::write(path, out).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

/// Port of `process_stylesheet_path`.
pub fn process_stylesheet_path(path: &Path, opts: &Options) -> Result<()> {
    if !opts.needs_stylesheet_processing() {
        return Ok(());
    }
    let css = fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let ncss = process_stylesheet(&css, opts);
    if ncss != css {
        fs::write(path, ncss).with_context(|| format!("Failed to write {}", path.display()))?;
    }
    Ok(())
}

/// Port of `process_path`.
pub fn process_path(path: &Path, kobo_js_href: &str, metadata_lang: &str, opts: &Options, media_type: &str) -> Result<()> {
    if OEB_DOCS.contains(&media_type) {
        kepubify_html_path(path, kobo_js_href, metadata_lang, opts)?;
    } else if OEB_STYLES.contains(&media_type) {
        process_stylesheet_path(path, opts)?;
    }
    Ok(())
}

/// Port of `do_work_in_parallel`. See this section's module docs for
/// why worker sizing doesn't reproduce `calculate_number_of_workers`.
pub fn do_work_in_parallel(container: &mut Container, kobo_js_name: &str, opts: &Options, metadata_lang: &str, max_workers: usize) -> Result<()> {
    let names_that_need_work: Vec<String> = container
        .base
        .mime_map
        .iter()
        .filter(|(_, mt)| OEB_DOCS.contains(&mt.as_str()) || OEB_STYLES.contains(&mt.as_str()))
        .map(|(n, _)| n.clone())
        .collect();

    let mut tasks: Vec<(std::path::PathBuf, String, String)> = Vec::with_capacity(names_that_need_work.len());
    for name in &names_that_need_work {
        let href = container.name_to_href(kobo_js_name, Some(name));
        let media_type = container.base.mime_map.get(name).cloned().unwrap_or_default();
        let path = container.get_file_path_for_processing(name, true)?;
        tasks.push((path, href, media_type));
    }

    let num_workers = if max_workers > 0 {
        max_workers
    } else {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .min(tasks.len().max(1))
    };

    if num_workers < 2 {
        for (path, href, media_type) in &tasks {
            process_path(path, href, metadata_lang, opts, media_type)?;
        }
        return Ok(());
    }

    let queue = Mutex::new(tasks.into_iter());
    let errors: Mutex<Vec<anyhow::Error>> = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..num_workers {
            let queue = &queue;
            let errors = &errors;
            scope.spawn(move || loop {
                let task = { queue.lock().unwrap().next() };
                let Some((path, href, media_type)) = task else { break };
                if let Err(e) = process_path(&path, &href, metadata_lang, opts, &media_type) {
                    errors.lock().unwrap().push(e);
                }
            });
        }
    });
    if let Some(e) = errors.into_inner().unwrap().into_iter().next() {
        return Err(e);
    }
    Ok(())
}

/// Port of `remove_kobo_files`.
pub fn remove_kobo_files(container: &mut Container) -> Result<()> {
    let entries: Vec<(String, String)> = container.base.mime_map.iter().map(|(n, mt)| (n.clone(), mt.clone())).collect();
    for (name, mt) in entries {
        let fname = name.rsplit('/').next().unwrap_or(&name);
        if (mt == "application/javascript" && fname == KOBO_JS_NAME) || (mt == "text/css" && fname == KOBO_CSS_NAME) {
            container.remove_item(&name, true)?;
        }
    }
    Ok(())
}

fn bytes_contains(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

/// Port of `check_for_kobo_drm`.
pub fn check_for_kobo_drm(container: &mut Container) -> Result<()> {
    if !container.has_name_and_is_not_empty("rights.xml") {
        return Ok(());
    }
    for (name, _is_linear) in container.spine_names()? {
        let mt = container.base.mime_map.get(&name).cloned().unwrap_or_default();
        if !OEB_DOCS.contains(&mt.as_str()) {
            continue;
        }
        let raw = container.read_file(&name)?;
        let head = &raw[..raw.len().min(8192)];
        if !bytes_contains(head, b"<?xml") && !bytes_contains(head, b"<html") && !bytes_contains(head, KOBO_SPAN_CLASS.as_bytes()) {
            return Err(PolishError::drm().into());
        }
        break;
    }
    Ok(())
}

/// Port of `uniqify_name`. Real upstream has a genuine bug in its retry
/// branch: `f'{uuid4()}/fname'` is a literal string `"fname"` with no
/// interpolation (missing braces around the parameter), not
/// `f'{uuid4()}/{fname}'` -- so a collision retry produces a path like
/// `<uuid>/fname` regardless of the actual requested name. Replicated
/// bug-for-bug, matching this project's own established precedent
/// (issue #653's disclosed bugs) of porting real observed behavior
/// rather than the obviously-intended one.
pub fn uniqify_name(container: &mut Container, fname: &str) -> Result<String> {
    let mut q = fname.to_string();
    while container.has_name_case_insensitive(&q) || container.manifest_has_name(&q)? {
        q = format!("{}/fname", calibre_utils::short_uuid::uuid4());
    }
    Ok(q)
}

fn container_mi(container: &mut Container) -> Result<MetaInformation> {
    let opf_bytes = container.opf()?.serialize();
    crate::opf::parse_opf(&String::from_utf8_lossy(&opf_bytes)).map_err(|e| anyhow::anyhow!("parsing OPF metadata: {e}"))
}

/// Port of `unkepubify_container`.
pub fn unkepubify_container(container: &mut Container, max_workers: usize) -> Result<()> {
    check_for_kobo_drm(container)?;
    remove_dummy_cover_image(container)?;
    remove_dummy_title_page(container)?;
    remove_kobo_files(container)?;
    let opts = Options { for_removal: true, ..Default::default() };
    let mi = container_mi(container)?;
    let metadata_lang = mi.languages.first().cloned().unwrap_or_default();
    do_work_in_parallel(container, KOBO_JS_NAME, &opts, &metadata_lang, max_workers)
}

/// Port of `kepubify_container`. `fontdb` backs the last-resort
/// synthesized cover path (`covers::generate_cover`, issue #116) for
/// books with no cover image at all -- callers typically build it once
/// via `covers_text::load_system_fonts()` and share it across calls.
pub fn kepubify_container(container: &mut Container, opts: &Options, max_workers: usize, fontdb: &Arc<fontdb::Database>) -> Result<()> {
    remove_dummy_title_page(container)?;
    remove_dummy_cover_image(container)?;
    remove_kobo_files(container)?;
    let mi = container_mi(container)?;

    let mut cover_image_name = find_cover_image(container, false)?;
    if cover_image_name.is_none() {
        cover_image_name = find_cover_image3(container)?;
    }
    let cover_image_name = match cover_image_name {
        Some(n) => n,
        None => {
            let cdata = generate_cover(&mi, &CoverPrefs::default(), fontdb, false)
                .ok_or_else(|| anyhow::anyhow!("Failed to generate a dummy cover image"))?;
            container.add_file(&format!("{DUMMY_COVER_IMAGE_NAME}.jpeg"), &cdata, None, None, true)?
        }
    };
    container.apply_unique_properties(Some(&cover_image_name), &["cover-image"])?;

    // Real upstream passes `suggested_id='js-kobo.js'` to `add_file`
    // here; this port's `add_file` (ported before #654, see its own
    // docs) has no `suggested_id` parameter, so the manifest item gets
    // an auto-assigned id (`id`/`id-1`/...) instead of `js-kobo.js`.
    // Purely cosmetic -- nothing dereferences a manifest item by id for
    // the kobo.js entry -- so not worth widening `add_file` for.
    let kobo_js_name = uniqify_name(container, KOBO_JS_NAME)?;
    let kobo_js_name = container.add_file(&kobo_js_name, kobo_js(), Some("application/javascript"), None, true)?;

    if find_cover_page(container)?.is_none() && !first_spine_item_is_probably_title_page(container)? {
        add_dummy_title_page_for(container, Some(&cover_image_name), &mi, &kobo_js_name)?;
    }
    let metadata_lang = mi.languages.first().cloned().unwrap_or_default();
    do_work_in_parallel(container, &kobo_js_name, opts, &metadata_lang, max_workers)
}

fn splitext_replace(path: &Path, new_ext: &str) -> std::path::PathBuf {
    path.with_extension(new_ext)
}

fn unique_outpath(path: &Path, base_outpath: std::path::PathBuf, allow_overwrite: bool, ext: &str) -> std::path::PathBuf {
    let mut outpath = base_outpath;
    if !allow_overwrite {
        let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        let mut c = 0u32;
        while outpath == path {
            c += 1;
            outpath = path.with_file_name(format!("{stem}-{c}.{ext}"));
        }
    }
    outpath
}

/// Port of `kepubify_path`.
pub fn kepubify_path(
    path: &Path,
    outpath: Option<&Path>,
    max_workers: usize,
    allow_overwrite: bool,
    opts: &Options,
    fontdb: &Arc<fontdb::Database>,
) -> Result<std::path::PathBuf> {
    let tdir = tempfile::tempdir()?;
    let mut any = get_container(path, tdir.path(), true)?;
    kepubify_container(any.as_container_mut(), opts, max_workers, fontdb)?;
    let outpath = unique_outpath(path, outpath.map(|p| p.to_path_buf()).unwrap_or_else(|| splitext_replace(path, "kepub")), allow_overwrite, "kepub");
    match &mut any {
        AnyContainer::Epub(c) => c.commit(Some(&outpath))?,
        AnyContainer::Kepub(c) => c.epub.commit(Some(&outpath))?,
        AnyContainer::Azw3(c) => c.commit(Some(&outpath))?,
    }
    Ok(outpath)
}

/// Port of `unkepubify_path`. Real upstream forces `ebook_cls=
/// EpubContainer` here so a `.kepub` input is opened as a plain
/// `EpubContainer` rather than through `KEPUBContainer`'s own
/// auto-unkepubify-on-open (which would double-unkepubify it). This
/// port has no `ebook_cls` override on [`get_container`] -- its one
/// real caller needing that behavior calls
/// [`EpubContainer::open_zip`]/[`EpubContainer::open_dir`] directly
/// instead of widening the generic dispatcher for a single use site.
pub fn unkepubify_path(path: &Path, outpath: Option<&Path>, max_workers: usize, allow_overwrite: bool) -> Result<std::path::PathBuf> {
    let tdir = tempfile::tempdir()?;
    let mut epub = if path.is_dir() {
        EpubContainer::open_dir(path, tdir.path())?
    } else {
        EpubContainer::open_zip(path, tdir.path())?
    };
    unkepubify_container(&mut epub.container, max_workers)?;
    let outpath = unique_outpath(path, outpath.map(|p| p.to_path_buf()).unwrap_or_else(|| splitext_replace(path, "epub")), allow_overwrite, "epub");
    epub.commit(Some(&outpath))?;
    Ok(outpath)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(remove_widows_and_orphans: bool, remove_at_page_rules: bool, for_removal: bool) -> Options {
        Options {
            remove_widows_and_orphans,
            remove_at_page_rules,
            for_removal,
            ..Default::default()
        }
    }

    #[test]
    fn nest_css_comments_escapes_close() {
        assert_eq!(nest_css_comments("a*/b"), "a*\u{200c}/b");
        assert_eq!(nest_css_comments("plain"), "plain");
    }

    #[test]
    fn leaves_css_untouched_when_no_flags_set() {
        let css = "p { color: red; }";
        assert_eq!(process_stylesheet(css, &opts(false, false, false)), css);
    }

    #[test]
    fn removal_short_circuits_without_cookie() {
        let css = "p { color: red; }";
        // for_removal with no cookie present must return the input
        // byte-for-byte (real upstream's own "avoid expensive parse"
        // fast path) even though `Stylesheet::parse` would reformat it.
        assert_eq!(process_stylesheet(css, &opts(false, false, true)), css);
    }

    fn has_page_rule(css: &str) -> bool {
        Stylesheet::parse(css).rules.iter().any(|r| matches!(r, Rule::Page(_)))
    }

    #[test]
    fn hides_and_restores_widows_and_orphans() {
        let css = "p { widows: 2; orphans: 3; color: red; }";
        let hidden = process_stylesheet(css, &opts(true, false, false));
        let hidden_sheet = Stylesheet::parse(&hidden);
        let style = hidden_sheet.style_rules().next().expect("style rule");
        assert!(style.style.get_property("widows").is_none());
        assert!(style.style.get_property("orphans").is_none());
        assert_eq!(
            style
                .style
                .get_property_value(&format!("-{CSS_COMMENT_COOKIE}-widows")),
            "2"
        );
        assert_eq!(
            style
                .style
                .get_property_value(&format!("-{CSS_COMMENT_COOKIE}-orphans")),
            "3"
        );
        assert!(hidden.contains(CSS_COMMENT_COOKIE));

        let restored = process_stylesheet(&hidden, &opts(false, false, true));
        let restored_sheet = Stylesheet::parse(&restored);
        let style = restored_sheet.style_rules().next().expect("style rule");
        assert_eq!(style.style.get_property_value("widows"), "2");
        assert_eq!(style.style.get_property_value("orphans"), "3");
        assert!(!restored.contains(CSS_COMMENT_COOKIE));
    }

    #[test]
    fn hides_and_restores_page_rules() {
        let css = "@page { margin: 1in; }\np { color: blue; }";
        let hidden = process_stylesheet(css, &opts(false, true, false));
        assert!(!has_page_rule(&hidden));
        assert!(hidden.contains(CSS_COMMENT_COOKIE));
        assert!(hidden.contains("color: blue"));

        let restored = process_stylesheet(&hidden, &opts(false, false, true));
        assert!(has_page_rule(&restored));
        assert!(restored.contains("margin: 1in"));
        assert!(!restored.contains(CSS_COMMENT_COOKIE));
    }

    #[test]
    fn hides_page_rule_with_selector_and_margin_box() {
        let css = "@page :first { margin-top: 2in; @top-left { content: \"hi\"; } }";
        let hidden = process_stylesheet(css, &opts(false, true, false));
        assert!(!has_page_rule(&hidden));

        let restored = process_stylesheet(&hidden, &opts(false, false, true));
        let sheet = Stylesheet::parse(&restored);
        let page = sheet
            .rules
            .iter()
            .find_map(Rule::as_page)
            .expect("restored @page rule");
        assert_eq!(page.selector.pseudo_class.as_deref(), Some("first"));
        assert_eq!(page.declarations.get_property_value("margin-top"), "2in");
        assert_eq!(page.margin_rules.len(), 1);
        assert_eq!(page.margin_rules[0].at_keyword, "@top-left");
    }

    #[test]
    fn removal_leaves_unrelated_css_alone_when_nothing_hidden() {
        // Cookie substring present (e.g. inside an unrelated string
        // value) but nothing this module itself hid -- must not crash
        // and must not fabricate a bogus restoration.
        let css = format!("p::before {{ content: \"{CSS_COMMENT_COOKIE}\"; }}");
        let restored = process_stylesheet(&css, &opts(false, false, true));
        assert!(restored.contains(CSS_COMMENT_COOKIE));
    }

    #[test]
    fn check_if_css_needs_modification_detects_both_flags() {
        assert_eq!(check_if_css_needs_modification(""), (false, false));
        assert_eq!(
            check_if_css_needs_modification("p { widows: 2; }"),
            (true, false)
        );
        assert_eq!(
            check_if_css_needs_modification("@page { margin: 1in; }"),
            (false, true)
        );
        assert_eq!(
            check_if_css_needs_modification("@page { margin: 1in; } p { orphans: 3; }"),
            (true, true)
        );
    }

    #[test]
    fn make_options_defaults_run_needs_modification_check() {
        let o = make_options(MakeOptionsArgs {
            extra_css: "@page { margin: 1in; }".to_string(),
            ..Default::default()
        });
        assert!(o.remove_at_page_rules);
        assert!(!o.remove_widows_and_orphans);
        assert!(o.hyphenation_css.is_empty());
        assert!(!o.for_removal);
    }

    #[test]
    fn make_options_explicit_flags_both_required() {
        // Real upstream quirk: providing only ONE of the two flags
        // still triggers a full recompute of BOTH from extra_css.
        let o = make_options(MakeOptionsArgs {
            extra_css: "@page { margin: 1in; }".to_string(),
            remove_widows_and_orphans: Some(true),
            remove_at_page_rules: None,
            ..Default::default()
        });
        assert!(o.remove_at_page_rules);
        assert!(!o.remove_widows_and_orphans);

        let o2 = make_options(MakeOptionsArgs {
            extra_css: "@page { margin: 1in; }".to_string(),
            remove_widows_and_orphans: Some(true),
            remove_at_page_rules: Some(false),
            ..Default::default()
        });
        assert!(o2.remove_widows_and_orphans);
        assert!(!o2.remove_at_page_rules);
    }

    #[test]
    fn make_options_hyphenation_disabled() {
        let o = make_options(MakeOptionsArgs {
            affect_hyphenation: true,
            disable_hyphenation: true,
            ..Default::default()
        });
        assert!(o.hyphenation_css.contains("hyphens: none !important"));
    }

    #[test]
    fn make_options_hyphenation_enabled_with_limits() {
        let o = make_options(MakeOptionsArgs {
            affect_hyphenation: true,
            hyphenation_min_chars: 7,
            hyphenation_min_chars_before: 2,
            hyphenation_min_chars_after: 4,
            hyphenation_limit_lines: 3,
            ..Default::default()
        });
        assert!(o.hyphenation_css.contains("hyphenate-limit-chars: 7 2 4"));
        assert!(o.hyphenation_css.contains("hyphenate-limit-lines: 3"));
        assert!(o.hyphenation_css.contains("-webkit-hyphens: auto"));
    }

    #[test]
    fn make_options_hyphenation_zero_min_chars_yields_empty_css() {
        let o = make_options(MakeOptionsArgs {
            affect_hyphenation: true,
            hyphenation_min_chars: 0,
            ..Default::default()
        });
        assert!(o.hyphenation_css.is_empty());
    }

    #[test]
    fn options_needs_stylesheet_processing() {
        assert!(!Options::default().needs_stylesheet_processing());
        assert!(opts(true, false, false).needs_stylesheet_processing());
        assert!(opts(false, true, false).needs_stylesheet_processing());
        assert!(opts(false, false, true).needs_stylesheet_processing());
    }

    fn html_root(dom: &Dom) -> NodeId {
        dom.find_first_tag_global("html").expect("html root")
    }

    #[test]
    fn add_and_remove_style_and_script_round_trip() {
        let mut dom = Dom::parse("<html><head></head><body></body></html>");
        let root = html_root(&dom);
        let o = Options {
            extra_css: "p { color: red; }".to_string(),
            ..Default::default()
        };
        assert!(add_style_and_script(&mut dom, root, "../kobo.js", &o));

        let head = dom.find_first_tag_global("head").unwrap();
        let styles = dom.find_all_tag(head, "style");
        assert_eq!(styles.len(), 2);
        assert_eq!(dom.node(styles[0]).attrs.get("id").map(String::as_str), Some(KOBO_CSS_ID));
        assert_eq!(dom.node(styles[1]).attrs.get("id").map(String::as_str), Some(EXTRA_CSS_ID));
        assert!(dom.text_content(styles[1]).contains("color: red"));
        let scripts = dom.find_all_tag(head, "script");
        assert_eq!(scripts.len(), 1);
        assert_eq!(dom.node(scripts[0]).attrs.get("src").map(String::as_str), Some("../kobo.js"));

        remove_kobo_styles_and_scripts(&mut dom, root);
        assert!(dom.find_all_tag(root, "style").is_empty());
        assert!(dom.find_all_tag(root, "script").is_empty());
    }

    #[test]
    fn add_style_and_script_falls_back_to_body_with_no_head() {
        // `Dom::parse` (HTML5 tag-soup, matching real
        // `parse_xhtml`) always auto-inserts a `<head>` per the HTML5
        // tree-construction algorithm even when the source omits one,
        // so exercising the no-head fallback branch needs a tree built
        // structurally rather than parsed from a headless string.
        let mut dom = Dom::empty();
        let html = dom.new_element("html");
        dom.append_child(dom.root, html);
        let body = dom.new_element("body");
        dom.append_child(html, body);

        assert!(dom.find_all_tag(html, "head").is_empty());
        assert!(add_style_and_script(&mut dom, html, "kobo.js", &Options::default()));
        assert_eq!(dom.find_all_tag(body, "script").len(), 1);
    }

    #[test]
    fn remove_kobo_styles_and_scripts_drops_kobo_css_link_and_comment() {
        let mut dom = Dom::parse(
            "<html><head><link rel=\"stylesheet\" type=\"text/css\" href=\"../kobo.css\"/></head>\
             <body><!-- kobo-style --><p>hi</p></body></html>",
        );
        let root = html_root(&dom);
        remove_kobo_styles_and_scripts(&mut dom, root);
        assert!(dom.find_all_tag(root, "link").is_empty());
        let mut comments = Vec::new();
        find_all_comments(&dom, root, &mut comments);
        assert!(comments.is_empty());
        // Unrelated content survives.
        assert!(dom.find_all_tag(root, "p").len() == 1);
    }

    #[test]
    fn wrap_and_unwrap_body_contents_round_trip() {
        let mut dom = Dom::parse("<html><body>Hello <b>world</b></body></html>");
        let body = dom.find_first_tag_global("body").unwrap();
        let inner = wrap_body_contents(&mut dom, body);

        let outer_div = dom.children(body);
        assert_eq!(outer_div.len(), 1);
        assert_eq!(dom.node(outer_div[0]).attrs.get("id").map(String::as_str), Some(OUTER_DIV_ID));
        assert_eq!(dom.parent(inner), Some(outer_div[0]));
        assert!(dom.text_content(inner).contains("Hello"));
        assert_eq!(dom.find_all_tag(inner, "b").len(), 1);

        unwrap_body_contents(&mut dom, body);
        assert!(dom.children(body).iter().all(|&c| dom.tag(c) != Some("div")));
        assert!(dom.text_content(body).contains("Hello"));
        assert_eq!(dom.find_all_tag(body, "b").len(), 1);
    }

    #[test]
    fn wrap_body_contents_only_strips_outer_div_id_not_inner_real_bug() {
        // Real upstream bug (see wrap_body_contents' own docs): a
        // pre-existing `id="book-inner"` on an unrelated element is
        // NOT stripped (missing quotes in the real XPath), only a
        // pre-existing `id="book-columns"` is.
        let mut dom = Dom::parse(
            "<html><body><div id=\"book-columns\">a</div><div id=\"book-inner\">b</div></body></html>",
        );
        let body = dom.find_first_tag_global("body").unwrap();
        wrap_body_contents(&mut dom, body);
        let mut with_inner_id = 0;
        let mut with_outer_id = 0;
        for elem in dom.preorder_elements(dom.root) {
            match dom.node(elem).attrs.get("id").map(String::as_str) {
                Some(INNER_DIV_ID) => with_inner_id += 1,
                Some(OUTER_DIV_ID) => with_outer_id += 1,
                _ => {}
            }
        }
        // Exactly one real book-columns/book-inner pair remains (the
        // freshly created wrapper) plus the STALE pre-existing
        // `id="book-inner"` div, which was never stripped.
        assert_eq!(with_outer_id, 1);
        assert_eq!(with_inner_id, 2);
    }

    #[test]
    fn is_probably_a_title_page_detects_cover_in_title_without_clearing_it() {
        let mut dom = Dom::parse("<html><head><title>Front Cover</title></head><body></body></html>");
        let root = html_root(&dom);
        assert!(is_probably_a_title_page(&mut dom, root));
        let title = dom.find_first_tag_global("title").unwrap();
        // The matching title's text is NOT cleared (real upstream
        // returns before reaching `title.text = None` for this one).
        assert_eq!(dom.text_content(title), "Front Cover");
    }

    #[test]
    fn is_probably_a_title_page_clears_non_matching_titles_then_measures_text() {
        let mut dom = Dom::parse(
            "<html><head><title>Chapter One</title></head><body><img src=\"a.png\"/></body></html>",
        );
        let root = html_root(&dom);
        // One image, no non-title text -> counts as a title page.
        assert!(is_probably_a_title_page(&mut dom, root));
        let title = dom.find_first_tag_global("title").unwrap();
        assert_eq!(dom.text_content(title), "");
    }

    #[test]
    fn is_probably_a_title_page_rejects_normal_chapter() {
        let mut dom = Dom::parse(
            "<html><body><p>This is a long paragraph with enough real prose in it to fail every title-page heuristic that exists.</p></body></html>",
        );
        let root = html_root(&dom);
        assert!(!is_probably_a_title_page(&mut dom, root));
    }

    fn write_test_epub(dir: &std::path::Path, extra_manifest: &str, extra_spine: &str) {
        std::fs::write(
            dir.join("content.opf"),
            format!(
                r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="2.0" unique-identifier="bookid">
  <metadata>
    <dc:title>Test Book</dc:title>
    <dc:identifier id="bookid">urn:uuid:12345678-1234-1234-1234-123456789012</dc:identifier>
  </metadata>
  <manifest>
    <item id="c1" href="chap1.html" media-type="application/xhtml+xml"/>
    {extra_manifest}
  </manifest>
  <spine>
    <itemref idref="c1"/>
    {extra_spine}
  </spine>
</package>"#
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join("chap1.html"),
            b"<html><body><h1>Chapter One</h1><p>This is a long paragraph with enough real prose \
in it to fail every title-page text-length heuristic that exists in this codebase.</p></body></html>",
        )
        .unwrap();
    }

    #[test]
    fn add_dummy_title_page_then_remove_it() {
        let dir = tempfile::tempdir().unwrap();
        write_test_epub(dir.path(), "", "");
        let mut c = crate::oeb::polish::container::Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        c.add_file(KOBO_JS_NAME, b"// kobo js", Some("application/javascript"), None, false)
            .unwrap();

        let mi = MetaInformation {
            title: "My Book".to_string(),
            authors: vec!["Jane Author".to_string()],
            ..Default::default()
        };
        let name = add_dummy_title_page_for(&mut c, None, &mi, KOBO_JS_NAME).unwrap();
        assert!(name.contains(DUMMY_TITLE_PAGE_NAME));

        let spine = c.spine_names().unwrap();
        assert_eq!(spine[0].0, name);

        let dom = c.get_xhtml(&name).unwrap();
        assert!(dom.text_content(dom.root).contains("My Book"));
        assert!(dom.text_content(dom.root).contains("Jane Author"));

        remove_dummy_title_page(&mut c).unwrap();
        let spine_after = c.spine_names().unwrap();
        assert!(spine_after.iter().all(|(n, _)| n != &name));
    }

    #[test]
    fn add_dummy_title_page_with_cover_image() {
        let dir = tempfile::tempdir().unwrap();
        write_test_epub(
            dir.path(),
            r#"<item id="cov" href="cover.png" media-type="image/png"/>"#,
            "",
        );
        let mut c = crate::oeb::polish::container::Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        std::fs::write(dir.path().join("cover.png"), b"\x89PNG\r\n\x1a\n").unwrap();
        c.add_file(KOBO_JS_NAME, b"// kobo js", Some("application/javascript"), None, false)
            .unwrap();

        let mi = MetaInformation::default();
        let name = add_dummy_title_page_for(&mut c, Some("cover.png"), &mi, KOBO_JS_NAME).unwrap();
        let dom = c.get_xhtml(&name).unwrap();
        let imgs = dom.find_all_tag_global("img");
        assert_eq!(imgs.len(), 1);
        assert!(dom.node(imgs[0]).attrs.get("src").unwrap().contains("cover.png"));
    }

    #[test]
    fn remove_dummy_cover_image_removes_matching_manifest_items() {
        let dir = tempfile::tempdir().unwrap();
        write_test_epub(dir.path(), "", "");
        let mut c = crate::oeb::polish::container::Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let name = c
            .add_file(&format!("{DUMMY_COVER_IMAGE_NAME}.png"), b"\x89PNG\r\n\x1a\n", Some("image/png"), None, false)
            .unwrap();
        assert!(c.base.mime_map.contains_key(&name));
        remove_dummy_cover_image(&mut c).unwrap();
        assert!(!c.base.mime_map.contains_key(&name));
    }

    #[test]
    fn first_spine_item_is_probably_title_page_by_name_and_by_content() {
        let dir = tempfile::tempdir().unwrap();
        write_test_epub(dir.path(), "", "");
        let mut c = crate::oeb::polish::container::Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        // chap1.html has substantial text -> not a title page by content.
        assert!(!first_spine_item_is_probably_title_page(&mut c).unwrap());
    }

    // ---- Kobo span wrapping (issue #651) ------------------------------
    //
    // Every assertion below was cross-validated against upstream's own
    // verbatim `add_kobo_spans`/`unwrap`/`remove_kobo_spans` running on a
    // minimal lxml-shaped tree (40 fixtures, byte-identical structural
    // dumps, `add_kobo_spans` and the `remove_kobo_spans` round trip
    // both) -- see this port's notes on the issue for the harness.

    fn spanned(html: &str, prefer_justification: bool) -> (Dom, NodeId) {
        let mut dom = Dom::parse(html);
        let inner = dom.find_by_id("i").expect("fixture needs id=i on the wrapper");
        add_kobo_spans(&mut dom, inner, "en", prefer_justification);
        (dom, inner)
    }

    /// Every `kobo.PARA.SEG` id in `html`, in document order.
    fn kobo_ids(html: &str) -> Vec<String> {
        html.match_indices("id=\"kobo.")
            .map(|(n, m)| {
                let rest = &html[n + m.len()..];
                rest[..rest.find('"').expect("id attribute is quoted")].to_string()
            })
            .collect()
    }

    fn spanned_html(html: &str, prefer_justification: bool) -> String {
        let (dom, inner) = spanned(html, prefer_justification);
        dom.serialize(inner)
    }

    #[test]
    fn splits_a_run_at_sentence_boundaries_numbering_each_segment() {
        assert_eq!(
            spanned_html("<div id=i><p>One. Two.</p></div>", false),
            "<div id=\"i\"><p>\
               <span class=\"koboSpan\" id=\"kobo.1.1\">One. </span>\
               <span class=\"koboSpan\" id=\"kobo.1.2\">Two.</span>\
             </p></div>"
                .replace(['\n'], "")
                .replace("               ", "")
                .replace("             ", "")
        );
    }

    #[test]
    fn leading_whitespace_is_pulled_inside_the_first_span() {
        // Not justifying: the run's leading whitespace goes *into* the
        // span, so Kobo's highlight has no unhighlighted gap in front.
        let html = spanned_html("<div id=i><p>  Hello there.</p></div>", false);
        assert!(html.contains(">  Hello there.</span>"), "{html}");
        assert!(!html.contains("<p>  <span"), "{html}");
    }

    #[test]
    fn prefer_justification_leaves_leading_whitespace_outside_the_span() {
        let html = spanned_html("<div id=i><p>  Hello there.</p></div>", true);
        assert!(html.contains("<p>  <span"), "{html}");
        assert!(html.contains(">Hello there.</span>"), "{html}");
    }

    #[test]
    fn prefer_justification_moves_trailing_whitespace_out_of_each_span() {
        let html = spanned_html("<div id=i><p>One.   Two.</p></div>", true);
        assert!(html.contains("<span class=\"koboSpan\" id=\"kobo.1.1\">One.</span>   "), "{html}");
        assert!(html.contains("<span class=\"koboSpan\" id=\"kobo.1.2\">Two.</span>"), "{html}");
    }

    #[test]
    fn a_block_holding_only_whitespace_still_gets_one_span() {
        // Upstream's own "block tag with only whitespace" branch: the
        // whole run, whitespace included, goes inside a span. Checked
        // before the pure-whitespace early return, so it still consumes
        // a paragraph number.
        assert_eq!(
            spanned_html("<div id=i><p>   </p></div>", false),
            "<div id=\"i\"><p><span class=\"koboSpan\" id=\"kobo.1.1\">   </span></p></div>"
        );
    }

    #[test]
    fn whitespace_between_blocks_is_left_exactly_as_it_was() {
        // Same pure-whitespace run, but with element siblings present,
        // so the branch above does not apply and no span is made.
        assert_eq!(
            spanned_html("<div id=i><p>a</p>   <p>b</p></div>", false),
            "<div id=\"i\">\
             <p><span class=\"koboSpan\" id=\"kobo.1.1\">a</span></p>   \
             <p><span class=\"koboSpan\" id=\"kobo.2.1\">b</span></p>\
             </div>"
                .replace("             ", "")
        );
    }

    #[test]
    fn nested_inline_elements_and_their_tails_are_wrapped_in_document_order() {
        let html = spanned_html("<div id=i><p>Start. <b>Bold text.</b> Tail here.</p></div>", false);
        assert_eq!(kobo_ids(&html), ["1.1", "1.2", "1.3"], "{html}");
        // The <b>'s own text is wrapped inside the <b>, not hoisted out.
        assert!(html.contains("<b><span class=\"koboSpan\" id=\"kobo.1.2\">Bold text.</span></b>"), "{html}");
    }

    #[test]
    fn an_image_is_wrapped_in_its_own_span_and_starts_a_new_paragraph() {
        // `wrap_child` bumps paranum unconditionally, so the run after
        // the image lands in that same new paragraph as segment 2.
        assert_eq!(
            spanned_html("<div id=i><p>Before. <img src=\"x.png\"> After.</p></div>", false),
            "<div id=\"i\"><p>\
               <span class=\"koboSpan\" id=\"kobo.1.1\">Before. </span>\
               <span class=\"koboSpan\" id=\"kobo.2.1\"><img src=\"x.png\" /></span>\
               <span class=\"koboSpan\" id=\"kobo.2.2\"> After.</span>\
             </p></div>"
                .replace("               ", "")
                .replace("             ", "")
        );
    }

    #[test]
    fn skipped_tags_are_not_descended_into_but_their_tails_still_are() {
        let html = spanned_html("<div id=i><pre>code here.</pre>after pre.</div>", false);
        assert!(html.contains("<pre>code here.</pre>"), "{html}");
        assert!(html.contains("<span class=\"koboSpan\" id=\"kobo.1.1\">after pre.</span>"), "{html}");
    }

    #[test]
    fn comments_survive_and_the_run_after_one_is_still_wrapped() {
        let html = spanned_html("<div id=i><p>a<!-- c -->tail</p></div>", false);
        assert!(html.contains("<!-- c -->"), "{html}");
        assert!(html.contains("id=\"kobo.1.1\">a</span>"), "{html}");
        assert!(html.contains("id=\"kobo.1.2\">tail</span>"), "{html}");
    }

    #[test]
    fn each_block_tag_starts_a_new_paragraph_number() {
        let html = spanned_html(
            "<div id=i><h1>Title.</h1><p>Body one. Body two.</p><ul><li>Item.</li></ul></div>",
            false,
        );
        assert_eq!(kobo_ids(&html), ["1.1", "2.1", "2.2", "3.1"], "{html}");
    }

    #[test]
    fn a_run_between_two_blocks_stays_in_the_preceding_paragraph() {
        // The bump only happens when the *next* block element is
        // reached, so loose text after a block joins the block's number.
        let html = spanned_html("<div id=i><h2>Head.</h2>loose. <h3>Sub.</h3></div>", false);
        assert_eq!(kobo_ids(&html), ["1.1", "1.2", "2.1"], "{html}");
    }

    #[test]
    fn lang_is_inherited_down_the_tree_and_overridden_per_element() {
        let dom = Dom::parse("<div id=i lang=\"fr\"><p><span lang=\"de\">x</span></p></div>");
        let outer = dom.find_by_id("i").unwrap();
        let p = dom.find_all_tag(outer, "p")[0];
        let inner_span = dom.find_all_tag(outer, "span")[0];
        assert_eq!(lang_for_elem(&dom, outer, "eng"), "fra");
        // No lang of its own -> inherits whatever was passed down.
        assert_eq!(lang_for_elem(&dom, p, "fra"), "fra");
        assert_eq!(lang_for_elem(&dom, inner_span, "fra"), "deu");
        // Unrecognized values fall back to the parent's, as upstream.
        let dom2 = Dom::parse("<p id=i lang=\"zzzz\">x</p>");
        let e = dom2.find_by_id("i").unwrap();
        assert_eq!(lang_for_elem(&dom2, e, "eng"), "eng");
    }

    #[test]
    fn remove_kobo_spans_round_trips_every_shape_back_to_the_original() {
        for fixture in [
            "<div id=i><p>One. Two.</p></div>",
            "<div id=i><p>  Hello there.</p></div>",
            "<div id=i><p>Start. <b>Bold text.</b> Tail here.</p></div>",
            "<div id=i><p>a</p>   <p>b</p></div>",
            "<div id=i><h1>Title.</h1><p>Body one. Body two.</p></div>",
            "<div id=i><p>A <b>b <i>c.</i> d</b> e.</p></div>",
            "<div id=i>Loose text. More.</div>",
        ] {
            let original = {
                let dom = Dom::parse(fixture);
                let inner = dom.find_by_id("i").unwrap();
                dom.serialize(inner)
            };
            let (mut dom, inner) = spanned(fixture, false);
            assert!(remove_kobo_spans(&mut dom, inner), "no spans found in {fixture}");
            assert_eq!(dom.serialize(inner), original, "round trip failed for {fixture}");
        }
    }

    #[test]
    fn remove_kobo_spans_reports_when_there_was_nothing_to_remove() {
        let mut dom = Dom::parse("<div id=i><p>plain <span class=\"other\">x</span></p></div>");
        let inner = dom.find_by_id("i").unwrap();
        assert!(!remove_kobo_spans(&mut dom, inner));
        // A span that isn't a Kobo span is left completely alone.
        assert!(dom.serialize(inner).contains("<span class=\"other\">x</span>"));
    }

    #[test]
    fn remove_kobo_spans_leaves_an_image_in_place() {
        let fixture = "<div id=i><p><img src=\"a.png\"></p></div>";
        let (mut dom, inner) = spanned(fixture, false);
        assert!(remove_kobo_spans(&mut dom, inner));
        assert_eq!(dom.serialize(inner), "<div id=\"i\"><p><img src=\"a.png\" /></p></div>");
    }

    #[test]
    fn unwrap_merges_its_text_into_the_preceding_run() {
        let mut dom = Dom::parse(
            "<p id=i>before<span class=\"koboSpan\" id=\"kobo.1.1\">mid</span>after</p>",
        );
        let p = dom.find_by_id("i").unwrap();
        let span = dom.find_all_tag(p, "span")[0];
        unwrap(&mut dom, span);
        // lxml keeps two text runs from ever being adjacent; so does this.
        assert_eq!(dom.serialize(p), "<p id=\"i\">beforemidafter</p>");
        assert_eq!(dom.node(p).children.len(), 1);
    }

    #[test]
    fn add_and_remove_kobo_markup_to_html_round_trips() {
        let source = "<html><head></head><body><p>One. Two.</p></body></html>";
        let mut dom = Dom::parse(source);
        let root = html_root(&dom);
        let opts = Options {
            extra_css: "p { color: red; }".to_string(),
            ..Default::default()
        };
        add_kobo_markup_to_html(&mut dom, root, "../kobo.js", &opts, "en");

        let marked = dom.serialize(root);
        assert!(marked.contains(&format!("id=\"{OUTER_DIV_ID}\"")), "{marked}");
        assert!(marked.contains(&format!("id=\"{INNER_DIV_ID}\"")), "{marked}");
        assert!(marked.contains("class=\"koboSpan\""), "{marked}");
        assert!(marked.contains(KOBO_CSS_ID), "{marked}");
        assert!(marked.contains("../kobo.js"), "{marked}");

        remove_kobo_markup_from_html(&mut dom, root);
        let cleaned = dom.serialize(root);
        assert!(!cleaned.contains("koboSpan"), "{cleaned}");
        assert!(!cleaned.contains(OUTER_DIV_ID), "{cleaned}");
        assert!(!cleaned.contains(KOBO_CSS_ID), "{cleaned}");
        assert!(cleaned.contains("<p>One. Two.</p>"), "{cleaned}");
    }

    #[test]
    fn first_spine_item_is_probably_title_page_short_circuits_on_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("content.opf"),
            br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="2.0" unique-identifier="bookid">
  <metadata>
    <dc:title>Test Book</dc:title>
    <dc:identifier id="bookid">urn:uuid:12345678-1234-1234-1234-123456789012</dc:identifier>
  </metadata>
  <manifest>
    <item id="c1" href="titlepage.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="c1"/>
  </spine>
</package>"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("titlepage.html"),
            b"<html><body>This body text is long enough that content-based heuristics alone would reject it as a title page.</body></html>",
        )
        .unwrap();
        let mut c = crate::oeb::polish::container::Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        assert!(first_spine_item_is_probably_title_page(&mut c).unwrap());
    }
}

#[cfg(test)]
mod orchestrator_tests {
    use super::*;
    use crate::oeb::polish::container::Container;
    use std::io::{self, Write as _};

    const COVER_JPEG: &[u8] = &[
        0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0x00, 0x01, 0x01, 0x00, 0x00,
        0x01, 0x00, 0x01, 0x00, 0x00, 0xFF, 0xD9,
    ];

    fn write_book(dir: &Path, with_cover: bool) {
        fs::create_dir_all(dir.join("META-INF")).unwrap();
        fs::write(
            dir.join("META-INF/container.xml"),
            br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#,
        )
        .unwrap();
        let cover_manifest = if with_cover {
            r#"<item id="cover" href="cover.jpeg" media-type="image/jpeg" properties="cover-image"/>"#
        } else {
            ""
        };
        fs::write(
            dir.join("content.opf"),
            format!(
                r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="3.0" unique-identifier="bookid">
  <metadata>
    <dc:title>Test Book</dc:title>
    <dc:creator>Jane Author</dc:creator>
    <dc:language>en</dc:language>
    <dc:identifier id="bookid">urn:uuid:12345678-1234-1234-1234-123456789012</dc:identifier>
  </metadata>
  <manifest>
    <item id="c1" href="chap1.xhtml" media-type="application/xhtml+xml"/>
    <item id="c2" href="chap2.xhtml" media-type="application/xhtml+xml"/>
    <item id="css" href="style.css" media-type="text/css"/>
    {cover_manifest}
  </manifest>
  <spine>
    <itemref idref="c1"/>
    <itemref idref="c2"/>
  </spine>
</package>"#
            ),
        )
        .unwrap();
        fs::write(
            dir.join("chap1.xhtml"),
            b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><head></head><body>\
              <p>The quick brown fox jumps over the lazy dog. It was a good jump.</p>\
              </body></html>",
        )
        .unwrap();
        fs::write(
            dir.join("chap2.xhtml"),
            b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><head></head><body>\
              <p>Second chapter text, also long enough not to look like a title page.</p>\
              </body></html>",
        )
        .unwrap();
        fs::write(
            dir.join("style.css"),
            b"p { widows: 2; orphans: 3; } @page { margin: 1in; }",
        )
        .unwrap();
        if with_cover {
            fs::write(dir.join("cover.jpeg"), COVER_JPEG).unwrap();
        }
    }

    fn test_fontdb() -> Arc<fontdb::Database> {
        Arc::new(crate::covers_text::load_system_fonts())
    }

    fn spine_html(container: &mut Container, name: &str) -> String {
        String::from_utf8(container.raw_data(name, false).unwrap()).unwrap()
    }

    #[test]
    fn kepubify_container_adds_markup_and_unkepubify_reverses_it() {
        let dir = tempfile::tempdir().unwrap();
        write_book(dir.path(), true);
        let mut container = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();

        // Real upstream's own default `Options()` leaves CSS untouched
        // (`needs_stylesheet_processing` is false unless a flag is set
        // explicitly, typically via `make_options`) -- set these
        // explicitly to also exercise the stylesheet-hiding path.
        let opts = Options { remove_at_page_rules: true, remove_widows_and_orphans: true, ..Default::default() };
        kepubify_container(&mut container, &opts, 1, &test_fontdb()).unwrap();

        // Kobo span/style/script markup landed in both content docs.
        let chap1 = spine_html(&mut container, "chap1.xhtml");
        assert!(chap1.contains(KOBO_SPAN_CLASS), "expected koboSpan markup, got: {chap1}");
        assert!(chap1.contains(KOBO_CSS_ID));
        assert!(chap1.contains(OUTER_DIV_ID) && chap1.contains(INNER_DIV_ID));

        // The existing cover image was reused (no dummy cover generated).
        assert!(!container.base.mime_map.keys().any(|n| n.contains(DUMMY_COVER_IMAGE_NAME)));
        // A cover *image* isn't a cover *page*: neither spine chapter is
        // one either (both are plain, longish paragraphs), so a dummy
        // title page pointing at the real cover image gets generated.
        assert!(container.base.mime_map.keys().any(|n| n.contains(DUMMY_TITLE_PAGE_NAME)));
        // A kobo.js file was added to the manifest.
        assert!(container.base.mime_map.values().any(|mt| mt == "application/javascript"));
        // @page/widows/orphans got hidden in the stylesheet. The raw
        // text can still contain the substrings "@page"/"widows" (the
        // hidden payload/property name embed them, see this module's
        // own docs on the disclosed comment-vs-marker-at-rule
        // narrowing) -- assert structurally instead, via a real parse.
        let css = spine_html(&mut container, "style.css");
        let parsed_css = Stylesheet::parse(&css);
        assert!(!parsed_css.rules.iter().any(|r| matches!(r, Rule::Page(_))), "still a live @page rule: {css}");
        let style_rule = parsed_css.style_rules().next().expect("one style rule");
        assert!(style_rule.style.get_property("widows").is_none());
        assert!(style_rule.style.get_property("orphans").is_none());

        unkepubify_container(&mut container, 1).unwrap();
        let chap1_after = spine_html(&mut container, "chap1.xhtml");
        assert!(!chap1_after.contains(KOBO_SPAN_CLASS));
        assert!(!chap1_after.contains(KOBO_CSS_ID));
        assert!(!chap1_after.contains(OUTER_DIV_ID));
        assert!(!container.base.mime_map.values().any(|mt| mt == "application/javascript"));
        assert!(!container.base.mime_map.keys().any(|n| n.contains(DUMMY_TITLE_PAGE_NAME)));
        let css_after = spine_html(&mut container, "style.css");
        assert!(css_after.contains("@page"));
        assert!(css_after.contains("widows"));
    }

    #[test]
    fn kepubify_container_generates_a_dummy_cover_when_none_exists() {
        let dir = tempfile::tempdir().unwrap();
        write_book(dir.path(), false);
        let mut container = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();

        kepubify_container(&mut container, &Options::default(), 1, &test_fontdb()).unwrap();

        let dummy_cover = container
            .base
            .mime_map
            .keys()
            .find(|n| n.contains(DUMMY_COVER_IMAGE_NAME))
            .cloned()
            .expect("a dummy cover should have been generated");
        assert!(!container.raw_data(&dummy_cover, false).unwrap().is_empty());

        unkepubify_container(&mut container, 1).unwrap();
        assert!(!container.base.mime_map.keys().any(|n| n.contains(DUMMY_COVER_IMAGE_NAME)));
    }

    #[test]
    fn check_for_kobo_drm_passes_when_no_rights_xml() {
        let dir = tempfile::tempdir().unwrap();
        write_book(dir.path(), true);
        let mut container = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        assert!(check_for_kobo_drm(&mut container).is_ok());
    }

    #[test]
    fn check_for_kobo_drm_rejects_locked_content() {
        let dir = tempfile::tempdir().unwrap();
        write_book(dir.path(), true);
        // rights.xml present + the spine's first doc has none of the
        // "not actually locked" markers real upstream checks for.
        fs::write(dir.path().join("rights.xml"), b"<rights/>").unwrap();
        fs::write(dir.path().join("chap1.xhtml"), vec![0u8; 64]).unwrap();
        let mut container = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let err = check_for_kobo_drm(&mut container).unwrap_err();
        assert!(err.to_string().to_lowercase().contains("drm"));
    }

    #[test]
    fn uniqify_name_replicates_the_missing_interpolation_bug() {
        let dir = tempfile::tempdir().unwrap();
        write_book(dir.path(), true);
        // Force a collision on the very first name checked.
        fs::write(dir.path().join(KOBO_JS_NAME), b"//existing").unwrap();
        let mut opf = fs::read_to_string(dir.path().join("content.opf")).unwrap();
        opf = opf.replace(
            "</manifest>",
            &format!(r#"<item id="kobojs" href="{KOBO_JS_NAME}" media-type="application/javascript"/></manifest>"#),
        );
        fs::write(dir.path().join("content.opf"), opf).unwrap();
        let mut container = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();

        let q = uniqify_name(&mut container, KOBO_JS_NAME).unwrap();
        assert!(q.ends_with("/fname"), "expected the literal, un-interpolated suffix, got: {q}");
    }

    #[test]
    fn kepubify_path_and_unkepubify_path_round_trip_a_real_epub_zip() {
        let src_dir = tempfile::tempdir().unwrap();
        write_book(src_dir.path(), true);
        // Pack the fixture directory into a real zip so `get_container`
        // dispatches through `EpubContainer::open_zip`, matching how a
        // real `.epub` file arrives on disk.
        let epub_path = src_dir.path().join("book.epub");
        {
            let file = fs::File::create(&epub_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let opts = zip::write::FileOptions::default();
            for entry in walkdir::WalkDir::new(src_dir.path()).into_iter().filter_map(|e| e.ok()) {
                if !entry.file_type().is_file() || entry.path() == epub_path {
                    continue;
                }
                let rel = entry.path().strip_prefix(src_dir.path()).unwrap().to_string_lossy().replace('\\', "/");
                zip.start_file(&rel, opts).unwrap();
                zip.write_all(&fs::read(entry.path()).unwrap()).unwrap();
            }
            zip.finish().unwrap();
        }

        let fontdb = test_fontdb();
        let kepub_path = kepubify_path(&epub_path, None, 1, false, &Options::default(), &fontdb).unwrap();
        assert!(kepub_path.exists());
        assert_ne!(kepub_path, epub_path);

        {
            let file = fs::File::open(&kepub_path).unwrap();
            let mut zip = zip::ZipArchive::new(file).unwrap();
            let mut chap1 = String::new();
            io::Read::read_to_string(&mut zip.by_name("chap1.xhtml").unwrap(), &mut chap1).unwrap();
            assert!(chap1.contains(KOBO_SPAN_CLASS));
        }

        let roundtrip_epub = unkepubify_path(&kepub_path, None, 1, false).unwrap();
        assert!(roundtrip_epub.exists());
        let file = fs::File::open(&roundtrip_epub).unwrap();
        let mut zip = zip::ZipArchive::new(file).unwrap();
        let mut chap1 = String::new();
        io::Read::read_to_string(&mut zip.by_name("chap1.xhtml").unwrap(), &mut chap1).unwrap();
        assert!(!chap1.contains(KOBO_SPAN_CLASS));
    }

    #[test]
    fn open_zip_as_kepub_unkepubifies_on_open_and_kepubifies_on_commit() {
        use crate::oeb::polish::container::{get_container, AnyContainer};

        let src_dir = tempfile::tempdir().unwrap();
        write_book(src_dir.path(), true);
        let epub_path = src_dir.path().join("book.epub");
        {
            let file = fs::File::create(&epub_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let opts = zip::write::FileOptions::default();
            for entry in walkdir::WalkDir::new(src_dir.path()).into_iter().filter_map(|e| e.ok()) {
                if !entry.file_type().is_file() || entry.path() == epub_path {
                    continue;
                }
                let rel = entry.path().strip_prefix(src_dir.path()).unwrap().to_string_lossy().replace('\\', "/");
                zip.start_file(&rel, opts).unwrap();
                zip.write_all(&fs::read(entry.path()).unwrap()).unwrap();
            }
            zip.finish().unwrap();
        }
        let fontdb = test_fontdb();
        // `get_container`'s dispatch (matching real upstream's own
        // `path.rpartition('.')[-1]`) keys off the literal final
        // extension, so the fixture needs to be named `*.kepub`
        // (`kepubify_path`'s own default outpath), not `*.kepub.epub`.
        let kepub_path = kepubify_path(&epub_path, None, 1, false, &Options::default(), &fontdb).unwrap();
        assert_eq!(kepub_path.extension().unwrap(), "kepub");

        // Opening it as a KEPUBContainer must present a *plain* EPUB
        // (spans already stripped) to the rest of `polish`.
        let open_tdir = tempfile::tempdir().unwrap();
        let mut any = get_container(&kepub_path, open_tdir.path(), true).unwrap();
        let AnyContainer::Kepub(kc) = &mut any else {
            panic!("expected .kepub.epub to dispatch to KepubContainer");
        };
        let chap1 = spine_html(&mut kc.epub.container, "chap1.xhtml");
        assert!(!chap1.contains(KOBO_SPAN_CLASS), "KEPUBContainer should unkepubify on open, got: {chap1}");

        // Committing a packed (non-dir) KepubContainer re-kepubifies a
        // *clone*, leaving `self` a plain, still-editable EPUB.
        let commit_tdir = tempfile::tempdir().unwrap();
        let final_path = commit_tdir.path().join("out.kepub.epub");
        kc.commit_epub(&final_path).unwrap();
        assert!(final_path.exists());
        let chap1_after_commit = spine_html(&mut kc.epub.container, "chap1.xhtml");
        assert!(!chap1_after_commit.contains(KOBO_SPAN_CLASS), "self must remain the plain, unkepubified copy");

        let file = fs::File::open(&final_path).unwrap();
        let mut zip = zip::ZipArchive::new(file).unwrap();
        let mut chap1_committed = String::new();
        io::Read::read_to_string(&mut zip.by_name("chap1.xhtml").unwrap(), &mut chap1_committed).unwrap();
        assert!(chap1_committed.contains(KOBO_SPAN_CLASS), "the committed output must be re-kepubified");
    }
}
