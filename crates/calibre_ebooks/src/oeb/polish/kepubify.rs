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

use anyhow::Result;

use crate::css::model::{Rule, Stylesheet, UnknownAtRule};
use crate::dom::{Dom, NodeId, NodeKind};
use crate::metadata::authors::authors_to_string;
use crate::metadata::meta::MetaInformation;
use crate::oeb::polish::container::{Container, ParsedItem};
use crate::oeb::polish::utils::insert_self_closing;

/// Port of `kepubify.py`'s `CSS_COMMENT_COOKIE`.
pub const CSS_COMMENT_COOKIE: &str = "calibre-removed-css-for-kobo";

pub const KOBO_CSS_ID: &str = "kobostylehacks";
pub const EXTRA_CSS_ID: &str = "kepubify-extra-css";
pub const EXTRA_KOBO_CSS_IDS: &[&str] = &["koboSpanStyle"];
pub const KOBO_JS_NAME: &str = "kobo.js";
pub const KOBO_CSS_NAME: &str = "kobo.css";
pub const OUTER_DIV_ID: &str = "book-columns";
pub const INNER_DIV_ID: &str = "book-inner";
/// Used by `add_kobo_spans` (issue #651, not yet ported).
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
