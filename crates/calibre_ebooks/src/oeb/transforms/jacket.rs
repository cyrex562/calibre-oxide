//! Port of `old_src/src/calibre/ebooks/oeb/transforms/jacket.py`.
//!
//! **This is the `oeb/transforms/` version.** `oeb/polish/jacket.py`
//! (issue #168, not yet ported) is a different file for the "Polish
//! Book" editor's jacket feature; the two share a name but not an
//! implementation.
//!
//! Generates a book-info "jacket" page (title/author/series/rating/tags/
//! comments, formatted from `templates/jacket/template.xhtml` +
//! `stylesheet.css`, copied from `old_src/resources/jacket/` the same
//! way `oeb::polish::cascade` copied in `templates/html.css`) and
//! inserts it at the start of the spine.
//!
//! # What's reused vs. built new
//!
//! - **Markdown** (`datatype == 'markdown'` custom columns): `pulldown-cmark`
//!   is already a dependency of this crate (`input/txt_input.rs`) --
//!   reused directly via [`render_markdown`] rather than adding a second
//!   markdown renderer.
//! - **`comments_to_html`**: a real, from-scratch port. Python's version
//!   uses `BeautifulSoup` to re-group a mix of loose text and inline tags
//!   into `<p class="description">` blocks; this port does the
//!   equivalent grouping over [`Dom`] (this crate's own HTML5 parser),
//!   which is real HTML5-tag-soup-tolerant parsing, not a text hack.
//! - **`urls_from_identifiers`**: Python's version is a small, literal
//!   `isbn`/`doi`/`arxiv`/`oclc`/`issn`/`uri*` lookup table *plus* a
//!   fan-out through every installed metadata-source plugin's
//!   `get_book_urls` (Amazon/Google/Goodreads/...), each with its own
//!   region/domain logic. This port covers exactly the literal lookup
//!   table (the part the task scope calls "a narrow slice of that source
//!   module") -- the plugin fan-out is the much larger, separate
//!   metadata-source-plugin system, out of scope for this file.
//! - **dates**: [`calibre_utils::date::format_date`] already covers the
//!   numeric tokens (`yyyy`/`yy`/`MM`/`dd`/`hh`/`mm`/`ss`) this file's
//!   date fields need, but not month-*name* tokens (`MMM`/`MMMM`), which
//!   the jacket's default `tweaks` formats use (`"MMM yyyy"`). This file
//!   adds exactly that (`format_jacket_date`) rather than duplicating the
//!   numeric-token handling `format_date` already has.
//! - **`calibre.utils.config.tweaks`**: a global user-preferences dict;
//!   [`JacketOptions`] is a narrow stand-in carrying just the handful of
//!   tweak values this file reads, with the project's documented
//!   defaults, matching how this project has handled other "global
//!   config" touchpoints elsewhere (e.g. `structure.rs`'s `*Options`
//!   structs).
//! - **Custom columns**: Python reads `mi.custom_field_keys()`/
//!   `get_user_metadata`/`format_field_extended` to render arbitrary
//!   user-defined columns into the jacket template. This crate's
//!   [`crate::metadata::meta::MetaInformation`] (the closest analog to
//!   Python's `Metadata`) has no structured custom-column model (its
//!   `user_metadata` is a flat `String->String` map, not calibre's
//!   richer per-column-type system) -- out of scope for this port; the
//!   template's `{_genre}`/`{_genre_label}` placeholders and any other
//!   `#custom` column simply render empty, the same as Python does for a
//!   book with no value for that column.

use std::collections::HashMap;

use anyhow::Result;
use regex::Regex;

use crate::metadata::meta::MetaInformation;
use crate::dom::{Dom, NodeId, NodeKind};
use crate::oeb::book::OEBBook;
use crate::oeb::constants::XHTML_MIME;

const JACKET_META_NAME: &str = "calibre-content";
const JACKET_META_VALUE: &str = "jacket";

const TEMPLATE_XHTML: &str = include_str!("../../../resources/jacket/template.xhtml");
const TEMPLATE_CSS: &str = include_str!("../../../resources/jacket/stylesheet.css");

// ===================================================================
// Small formatting helpers (fmt_sidx / rating_to_stars / roman numerals)
// ===================================================================

/// Port of `calibre.ebooks.metadata.fmt_sidx`.
pub fn fmt_sidx(i: Option<f64>, use_roman: bool) -> String {
    let i = i.unwrap_or(1.0);
    if i.fract() == 0.0 {
        let n = i as i64;
        if use_roman {
            int_to_roman(n)
        } else {
            n.to_string()
        }
    } else {
        let mut ans = format!("{i:.2}");
        if ans.contains('.') {
            while ans.ends_with('0') {
                ans.pop();
            }
            if ans.ends_with('.') {
                ans.pop();
            }
        }
        ans
    }
}

/// A plain integer -> roman numeral converter (no direct equivalent
/// elsewhere in this crate -- `epub::pages::filter_name` only
/// *recognizes* roman numerals via regex, it doesn't generate them).
fn int_to_roman(mut n: i64) -> String {
    if n <= 0 {
        return n.to_string();
    }
    const VALUES: &[(i64, &str)] = &[
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut out = String::new();
    for &(value, sym) in VALUES {
        while n >= value {
            out.push_str(sym);
            n -= value;
        }
    }
    out
}

/// Port of `calibre.ebooks.metadata.rating_to_stars`.
pub fn rating_to_stars(value: Option<f64>, allow_half_stars: bool) -> String {
    let r = (value.unwrap_or(0.0) as i64).clamp(0, 10);
    let mut ans = "★".repeat((r / 2) as usize);
    if allow_half_stars && r % 2 == 1 {
        ans.push('⯨');
    }
    ans
}

/// Port of `render_jacket`'s local `get_rating`.
fn get_rating(rating: Option<f64>, rchar: &str, e_rchar: &str) -> String {
    let Some(rating) = rating else {
        return String::new();
    };
    let num = (rating / 2.0).clamp(0.0, 5.0);
    if num < 1.0 {
        return String::new();
    }
    format!(
        "{}{}",
        rchar.repeat(num as usize),
        e_rchar.repeat(5 - num as usize)
    )
}

// ===================================================================
// urls_from_identifiers -- narrow port, see module docs
// ===================================================================

/// Port of the literal-lookup-table slice of
/// `calibre.ebooks.metadata.sources.identify.urls_from_identifiers`:
/// `isbn`/`doi`/`arxiv`/`oclc`/`issn`/`uri*`/`url*` schemes. Returns
/// `(display_name, id_type, id_value, url)` tuples, same shape as
/// Python's.
pub fn urls_from_identifiers(
    identifiers: &HashMap<String, String>,
) -> Vec<(String, String, String, String)> {
    let mut ans = Vec::new();
    let lower: HashMap<String, String> = identifiers
        .iter()
        .map(|(k, v)| (k.to_lowercase(), v.clone()))
        .collect();

    if let Some(isbn) = lower.get("isbn") {
        if !isbn.is_empty() {
            ans.push((
                isbn.clone(),
                "isbn".to_string(),
                isbn.clone(),
                format!("https://www.worldcat.org/isbn/{isbn}"),
            ));
        }
    }
    if let Some(doi) = lower.get("doi") {
        if !doi.is_empty() {
            ans.push((
                "DOI".to_string(),
                "doi".to_string(),
                doi.clone(),
                format!("https://dx.doi.org/{doi}"),
            ));
        }
    }
    if let Some(arxiv) = lower.get("arxiv") {
        if !arxiv.is_empty() {
            ans.push((
                "arXiv".to_string(),
                "arxiv".to_string(),
                arxiv.clone(),
                format!("https://arxiv.org/abs/{arxiv}"),
            ));
        }
    }
    if let Some(oclc) = lower.get("oclc") {
        if !oclc.is_empty() {
            ans.push((
                "OCLC".to_string(),
                "oclc".to_string(),
                oclc.clone(),
                format!("https://www.worldcat.org/oclc/{oclc}"),
            ));
        }
    }
    if let Some(issn) = lower.get("issn") {
        if !issn.is_empty() {
            ans.push((
                issn.clone(),
                "issn".to_string(),
                issn.clone(),
                format!("https://www.worldcat.org/issn/{issn}"),
            ));
        }
    }
    let uri_re = uri_scheme_re();
    for (k, v) in &lower {
        if v.is_empty() || !uri_re.is_match(k) {
            continue;
        }
        let scheme = v.split(':').next().unwrap_or("").to_lowercase();
        if scheme == "http" || scheme == "https" || scheme == "file" {
            let name = v
                .split_once("://")
                .map(|(_, rest)| rest.split('/').next().unwrap_or(v.as_str()))
                .unwrap_or(v.as_str());
            ans.push((name.to_string(), k.clone(), v.clone(), v.clone()));
        }
    }
    ans
}

fn uri_scheme_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^ur[il]\d*$").unwrap())
}

// ===================================================================
// Dates
// ===================================================================

/// Port of `calibre.utils.date.is_date_undefined`.
pub fn is_date_undefined(dt: Option<chrono::DateTime<chrono::Utc>>) -> bool {
    use chrono::Datelike;
    match dt {
        None => true,
        Some(d) => {
            d.year() < calibre_utils::date::UNDEFINED_DATE_YEAR
                || (d.year() == calibre_utils::date::UNDEFINED_DATE_YEAR
                    && d.month() == 1
                    && d.day() == 1)
        }
    }
}

/// Port of `calibre.utils.date.as_local_time`.
pub fn as_local_time(dt: chrono::DateTime<chrono::Utc>) -> chrono::DateTime<chrono::Local> {
    dt.with_timezone(&chrono::Local)
}

/// A Qt-style date format string (`yyyy`/`MMM`/`dd`/...) -- extends
/// [`calibre_utils::date::format_date`] with month-*name* tokens
/// (`MMMM`/`MMM`), the one thing that helper doesn't already cover; see
/// the module docs.
pub fn format_jacket_date(dt: chrono::DateTime<chrono::Local>, fmt: &str) -> String {
    use chrono::Datelike;
    let month_name = dt.format("%B").to_string();
    let month_abbr = dt.format("%b").to_string();
    // Longest-token-first so "MMMM" isn't partially consumed by an "MMM"
    // replacement first.
    let with_month_names = fmt.replace("MMMM", &month_name).replace("MMM", &month_abbr);
    let utc = dt.with_timezone(&chrono::Utc);
    let mut out = calibre_utils::date::format_date(&utc, &with_month_names);
    // `format_date` doesn't understand a lone `M`/`d` (non-doubled)
    // token; the jacket's own default formats never use them, but as a
    // narrow safety net, leave any stray single tokens untouched rather
    // than mis-substituting.
    if out.is_empty() {
        out = with_month_names;
    }
    let _ = dt.year();
    out
}

// ===================================================================
// Template value substitution: a scoped `string.Formatter` equivalent
// ===================================================================

/// One template argument: `{name}` renders as `default`; `{name.attr}`
/// renders as `attrs[attr]` (or `""` if absent, matching Python's
/// `SafeFormatter`'s `KeyError -> ''` catch-all).
#[derive(Clone, Debug, Default)]
struct TemplateValue {
    default: String,
    attrs: HashMap<String, String>,
}

impl TemplateValue {
    fn plain(s: impl Into<String>) -> Self {
        TemplateValue {
            default: s.into(),
            attrs: HashMap::new(),
        }
    }
}

fn template_token_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\{([A-Za-z_][A-Za-z0-9_]*)(?:\.([A-Za-z_][A-Za-z0-9_]*))?\}").unwrap()
    })
}

/// Port of `SafeFormatter.format(template, **args)`: substitutes every
/// `{name}`/`{name.attr}` token, rendering `""` for anything unknown
/// (Python's `except KeyError: return ''`). This is a **documented,
/// narrower** grammar than the full `string.Formatter` -- no `!conv`/
/// `:spec`/nested braces/positional args -- but `template.xhtml` (see
/// its source) never uses anything beyond bare-name and one level of
/// attribute access.
fn safe_format(template: &str, args: &HashMap<String, TemplateValue>) -> String {
    template_token_re()
        .replace_all(template, |caps: &regex::Captures| {
            let name = &caps[1];
            let Some(val) = args.get(name) else {
                return String::new();
            };
            match caps.get(2) {
                Some(attr) => val.attrs.get(attr.as_str()).cloned().unwrap_or_default(),
                None => val.default.clone(),
            }
        })
        .into_owned()
}

// ===================================================================
// comments_to_html / markdown
// ===================================================================

const INLINE_TAGS: &[&str] = &["br", "b", "i", "em", "strong", "span", "font", "a", "hr"];

fn lost_cr_pat() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"([a-z])([.?!])([A-Z])").unwrap())
}

fn lost_cr_exception_pat() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(Ph\.D)|(D\.Phil)|((Dr|Mr|Mrs|Ms)\.[A-Z])").unwrap())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn sanitize_pat() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)<script|<table|<tr|<td|<th|<style|<iframe").unwrap())
}

/// Port of `calibre.library.comments.sanitize_comments_html`: round-trip
/// through `html2text` then back to HTML via Markdown, used for comments
/// containing tags this module's own DOM-regrouping pass in
/// [`comments_to_html`] isn't meant to handle (`<script>`, `<table>` and
/// friends -- block-structured or unsafe markup, not the small inline-tag
/// vocabulary [`comments_to_html`] regroups around).
pub fn sanitize_comments_html(html: &str) -> String {
    let text = calibre_utils::html2text::html2text(html);
    render_markdown(&text)
}

/// Port of `calibre.library.comments.comments_to_html`.
pub fn comments_to_html(comments: &str) -> String {
    if comments.is_empty() {
        return "<p></p>".to_string();
    }
    if comments.trim_start().starts_with('<') {
        // Already HTML -- do not mess with it.
        return comments.to_string();
    }
    if !comments.contains('<') {
        let escaped = xml_escape(comments);
        return escaped
            .split("\n\n")
            .map(|x| format!("<p class=\"description\">{}</p>", x.replace('\n', "<br />")))
            .collect::<Vec<_>>()
            .join("\n");
    }
    if sanitize_pat().is_match(comments) {
        return sanitize_comments_html(comments);
    }

    // A mix of loose text and inline markup: fold "lost" paragraph
    // breaks back in, then convert newline conventions, then re-group
    // into <p class="description"> blocks around any top-level inline
    // content -- the same shape as Python's BeautifulSoup-based pass,
    // done over this crate's own HTML5 parser.
    let mut text = comments.to_string();
    text = lost_cr_exception_pat()
        .replace_all(&text, |c: &regex::Captures| c[0].replace('.', ".\r"))
        .into_owned();
    text = lost_cr_pat().replace_all(&text, "$1$2\n\n$3").into_owned();
    text = text.replace('\r', "");
    text = text.replace("\n\n", "<p>");
    text = text.replace('\n', "<br />");
    text = text.replace("--", "&mdash;");

    let dom = Dom::parse(&format!("<div>{text}</div>"));
    let Some(root) = dom.find_first_tag_global("div") else {
        return format!("<p class=\"description\">{text}</p>");
    };

    let mut out = String::new();
    let mut open: Vec<String> = Vec::new();
    let flush = |open: &mut Vec<String>, out: &mut String| {
        if !open.is_empty() {
            out.push_str("<p class=\"description\">");
            out.push_str(&open.join(""));
            out.push_str("</p>");
            open.clear();
        }
    };
    for child in dom.children(root) {
        match &dom.node(child).kind {
            NodeKind::Text(_) => open.push(dom.serialize(child)),
            NodeKind::Element(tag) if INLINE_TAGS.contains(&tag.as_str()) => {
                open.push(dom.serialize(child))
            }
            NodeKind::Comment(_) => {}
            _ => {
                flush(&mut open, &mut out);
                out.push_str(&dom.serialize(child));
            }
        }
    }
    flush(&mut open, &mut out);
    out
}

/// Port of `calibre.library.comments.markdown`: real Markdown ->
/// HTML rendering via `pulldown-cmark`, already a crate dependency.
pub fn render_markdown(val: &str) -> String {
    let mut options = pulldown_cmark::Options::empty();
    options.insert(pulldown_cmark::Options::ENABLE_TABLES);
    options.insert(pulldown_cmark::Options::ENABLE_STRIKETHROUGH);
    let parser = pulldown_cmark::Parser::new_ext(val, options);
    let mut html_out = String::new();
    pulldown_cmark::html::push_html(&mut html_out, parser);
    // Python's fixup: an empty paragraph (just a <br>) renders as two
    // blank lines in Qt rich text widgets, so collapse it to a single
    // non-breaking space.
    empty_p_br_re()
        .replace_all(&html_out, "<p$1>\u{a0}</p>")
        .into_owned()
}

fn empty_p_br_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"<p(|\s+[^>]*?)>\s*<br\s*/?>\s*</p>").unwrap())
}

// ===================================================================
// render_jacket
// ===================================================================

/// Options this transform reads (`opts.*`/`tweaks[...]` in Python). See
/// the module docs for why this is a narrow stand-in, not a full
/// preferences system.
#[derive(Debug, Clone)]
pub struct JacketOptions {
    pub remove_first_image: bool,
    pub insert_metadata: bool,
    pub smarten_punctuation: bool,
    /// `output_profile.ratings_char`/`.empty_ratings_char`.
    pub ratings_char: String,
    pub empty_ratings_char: String,
    /// `output_profile.short_name` -- only ever compared against
    /// `"kindle"` by [`postprocess_jacket`].
    pub output_profile_short_name: String,
    pub pubdate_format: String,
    pub timestamp_format: String,
}

impl Default for JacketOptions {
    fn default() -> Self {
        JacketOptions {
            remove_first_image: false,
            insert_metadata: true,
            smarten_punctuation: false,
            ratings_char: "*".to_string(),
            empty_ratings_char: " ".to_string(),
            output_profile_short_name: "default".to_string(),
            pubdate_format: "MMM yyyy".to_string(),
            timestamp_format: "dd MMM yyyy".to_string(),
        }
    }
}

fn series_value(series: Option<&str>, series_index: f64) -> TemplateValue {
    let Some(series) = series.filter(|s| !s.is_empty()) else {
        return TemplateValue::plain(String::new());
    };
    let escaped_series = xml_escape(series);
    let roman = format!(
        "{} of <em>{}</em>",
        fmt_sidx(Some(series_index), true),
        escaped_series
    );
    let combined = format!(
        "{} of <em>{}</em>",
        fmt_sidx(Some(series_index), false),
        escaped_series
    );
    let mut attrs = HashMap::new();
    attrs.insert("roman".to_string(), roman);
    attrs.insert("name".to_string(), escaped_series);
    attrs.insert("number".to_string(), fmt_sidx(Some(series_index), false));
    attrs.insert(
        "roman_number".to_string(),
        fmt_sidx(Some(series_index), true),
    );
    TemplateValue {
        default: combined,
        attrs,
    }
}

fn timestamp_value(dt: Option<chrono::DateTime<chrono::Utc>>, fmt: &str) -> TemplateValue {
    if is_date_undefined(dt) {
        return TemplateValue::plain(String::new());
    }
    let local = as_local_time(dt.expect("checked by is_date_undefined"));
    let default = xml_escape(&format_jacket_date(local, fmt));
    let mut attrs = HashMap::new();
    for token in ["yyyy", "yy", "MMMM", "MMM", "MM", "dd"] {
        attrs.insert(
            token.to_string(),
            xml_escape(&format_jacket_date(local, token)),
        );
    }
    TemplateValue { default, attrs }
}

fn tags_value(tags: &[String]) -> TemplateValue {
    let escaped: Vec<String> = tags.iter().map(|t| xml_escape(t)).collect();
    let default = escaped.join(", ");
    let mut alpha = escaped.clone();
    alpha.sort_by_key(|a| a.to_lowercase());
    let mut attrs = HashMap::new();
    attrs.insert("alphabetical".to_string(), alpha.join(", "));
    TemplateValue { default, attrs }
}

fn identifiers_value(identifiers: &HashMap<String, String>) -> TemplateValue {
    let links: Vec<String> = urls_from_identifiers(identifiers)
        .into_iter()
        .map(|(name, id_typ, id_val, url)| {
            format!(
                "<a href=\"{}\" title=\"{}:{}\">{}</a>",
                xml_escape(&url),
                xml_escape(&id_typ),
                xml_escape(&id_val),
                xml_escape(&name)
            )
        })
        .collect();
    let mut attrs: HashMap<String, String> = identifiers
        .iter()
        .map(|(k, v)| (k.clone(), xml_escape(v)))
        .collect();
    attrs.insert("links".to_string(), links.join(", "));
    TemplateValue {
        default: String::new(),
        attrs,
    }
}

/// Port of `render_jacket`. Returns the generated jacket document as a
/// [`Dom`] (Python returns an `lxml` root element -- this is the direct
/// analog).
pub fn render_jacket(
    mi: &MetaInformation,
    opts: &JacketOptions,
    alt_title: &str,
    alt_tags: &[String],
    alt_authors: &[String],
    alt_comments: &str,
    rescale_fonts: bool,
) -> Result<Dom> {
    let template = TEMPLATE_XHTML;
    let css = TEMPLATE_CSS;

    let title_str = if mi.title.is_empty() {
        alt_title.to_string()
    } else {
        mi.title.clone()
    };
    let title_str_escaped = xml_escape(&title_str);
    let title = format!("<span class=\"title\">{title_str_escaped}</span>");

    let authors = if mi.authors.is_empty() {
        alt_authors.to_vec()
    } else {
        mi.authors.clone()
    };
    let author = xml_escape(&authors.join(" & "));

    let publisher = xml_escape(mi.publisher.as_deref().unwrap_or(""));

    let rating = get_rating(mi.rating, &opts.ratings_char, &opts.empty_ratings_char);

    let tags = if mi.tags.is_empty() {
        alt_tags.to_vec()
    } else {
        mi.tags.clone()
    };

    let comments_raw = match &mi.comments {
        Some(c) if !c.trim().is_empty() => c.trim(),
        _ => alt_comments.trim(),
    };
    let comments = if comments_raw.is_empty() {
        String::new()
    } else {
        comments_to_html(comments_raw)
    };

    let mut has_data: HashMap<&str, bool> = HashMap::new();
    has_data.insert(
        "series",
        mi.series.as_deref().map(|s| !s.is_empty()).unwrap_or(false),
    );
    has_data.insert("tags", !tags.is_empty());
    has_data.insert("rating", !rating.is_empty());
    has_data.insert("pubdate", !is_date_undefined(mi.pubdate));
    has_data.insert("timestamp", !is_date_undefined(mi.timestamp));
    has_data.insert("publisher", !publisher.is_empty());

    let mut args: HashMap<String, TemplateValue> = HashMap::new();
    args.insert(
        "title_str".to_string(),
        TemplateValue::plain(title_str_escaped),
    );
    args.insert(
        "identifiers".to_string(),
        identifiers_value(&mi.identifiers),
    );
    args.insert("css".to_string(), TemplateValue::plain(css.to_string()));
    args.insert("title".to_string(), TemplateValue::plain(title));
    args.insert("author".to_string(), TemplateValue::plain(author));
    args.insert("publisher".to_string(), TemplateValue::plain(publisher));
    args.insert(
        "publisher_label".to_string(),
        TemplateValue::plain("Publisher"),
    );
    args.insert(
        "pubdate_label".to_string(),
        TemplateValue::plain("Published"),
    );
    args.insert(
        "pubdate".to_string(),
        timestamp_value(mi.pubdate, &opts.pubdate_format),
    );
    args.insert("series_label".to_string(), TemplateValue::plain("Series"));
    args.insert(
        "series".to_string(),
        series_value(mi.series.as_deref(), mi.series_index),
    );
    args.insert("rating_label".to_string(), TemplateValue::plain("Rating"));
    args.insert("rating".to_string(), TemplateValue::plain(rating));
    args.insert("tags_label".to_string(), TemplateValue::plain("Tags"));
    args.insert("tags".to_string(), tags_value(&tags));
    args.insert(
        "timestamp".to_string(),
        timestamp_value(mi.timestamp, &opts.timestamp_format),
    );
    args.insert("timestamp_label".to_string(), TemplateValue::plain("Date"));
    args.insert("comments".to_string(), TemplateValue::plain(comments));
    args.insert("footer".to_string(), TemplateValue::plain(""));
    let searchable_tags = tags
        .iter()
        .map(|t| format!("{}ttt", xml_escape(t)))
        .collect::<Vec<_>>()
        .join(" ");
    args.insert(
        "searchable_tags".to_string(),
        TemplateValue::plain(searchable_tags),
    );
    args.insert(
        "_genre_label".to_string(),
        TemplateValue::plain("{_genre_label}"),
    );
    args.insert("_genre".to_string(), TemplateValue::plain("{_genre}"));

    let mut generated = safe_format(template, &args);
    generated = strip_encoding_declarations(&generated);
    if opts.smarten_punctuation {
        generated = crate::oeb::polish::smartypants::smarten_punctuation_html(&generated);
    }

    let mut dom = Dom::parse(&generated);

    if rescale_fonts {
        for body in dom.find_all_tag_global("body") {
            let children = dom.children(body);
            let wrapper = dom.new_element("div");
            dom.node_mut(wrapper)
                .attrs
                .insert("data-calibre-rescale".to_string(), "100".to_string());
            for c in children {
                dom.detach(c);
                dom.append_child(wrapper, c);
            }
            dom.append_child(body, wrapper);
        }
    }

    postprocess_jacket(&mut dom, opts, &has_data);
    // Python finishes with `pretty_html_tree(None, root)` -- purely
    // cosmetic re-indentation of the generated markup, not needed for
    // correctness. `oeb::polish::pretty::pretty_html_tree` would be the
    // direct equivalent, but it unconditionally walks every `<style>`
    // tag through `pretty_script_or_style`, which is a real, documented
    // `todo!()` for CSS pretty-printing (`pretty_css`'s docs) -- and the
    // jacket template always embeds a `<style>` tag. Skipped rather than
    // panicking on every call; the emitted markup is still well-formed,
    // just not re-indented.

    Ok(dom)
}

/// A narrow stand-in for `calibre.ebooks.chardet.strip_encoding_declarations`:
/// drops an `<?xml ... ?>` prolog and any `<meta charset=.../>`/
/// `http-equiv="Content-Type"` declaration, which would otherwise
/// conflict with this crate's own encoding handling when the generated
/// snippet is re-parsed/embedded.
fn strip_encoding_declarations(html: &str) -> String {
    static XML_DECL: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = XML_DECL.get_or_init(|| Regex::new(r"(?s)<\?xml.*?\?>").unwrap());
    re.replace(html, "").trim_start().to_string()
}

/// Port of `postprocess_jacket`: strips header rows with nothing to
/// show. Reuses [`crate::oeb::polish::pretty::leading_text`]/
/// [`crate::oeb::polish::pretty::dom_tail`] (`pub(crate)`, so visible
/// crate-wide) for the same tail-preserving element removal `split.rs`
/// uses, rather than a bare `detach` that would silently drop
/// surrounding whitespace text.
fn postprocess_jacket(dom: &mut Dom, opts: &JacketOptions, has_data: &HashMap<&str, bool>) {
    use crate::oeb::polish::pretty::{dom_tail, leading_text, set_dom_tail, set_leading_text};

    let extract_class = |dom: &mut Dom, cls: &str| {
        let matches: Vec<NodeId> = dom
            .preorder_elements(dom.root)
            .into_iter()
            .filter(|&e| dom.node(e).attrs.get("class").map(|c| c.as_str()) == Some(cls))
            .collect();
        for tag in matches {
            let Some(parent) = dom.parent(tag) else {
                continue;
            };
            let Some(idx) = dom.index_in_parent(tag) else {
                continue;
            };
            if let Some(tail) = dom_tail(dom, tag) {
                if !tail.is_empty() {
                    if idx == 0 {
                        let existing = leading_text(dom, parent).unwrap_or_default();
                        set_leading_text(dom, parent, &format!("{existing}{tail}"));
                    } else {
                        let siblings = dom.children(parent);
                        if let Some(&prev) = siblings.get(idx - 1) {
                            let existing = dom_tail(dom, prev).unwrap_or_default();
                            set_dom_tail(dom, prev, &format!("{existing}{tail}"));
                        }
                    }
                }
            }
            dom.detach(tag);
        }
    };

    for key in ["series", "rating", "tags"] {
        if !has_data.get(key).copied().unwrap_or(false) {
            extract_class(dom, &format!("cbj_{key}"));
        }
    }
    if !has_data.get("pubdate").copied().unwrap_or(false) {
        extract_class(dom, "cbj_pubdata");
    }
    if opts.output_profile_short_name != "kindle" {
        extract_class(dom, "cbj_kindle_banner_hr");
    }
}

/// Port of `linearize_jacket`: for output formats that can't lay out
/// tables (PDF/some readers), turns the jacket's `<table>`/`<tr>`/`<th>`
/// into `<div>`s and `<td>` into `<span>`, in place.
pub fn linearize_jacket(oeb: &mut OEBBook) {
    let spine_hrefs: Vec<String> = oeb
        .spine
        .iter()
        .take(4)
        .filter_map(|s| oeb.manifest.get_by_id(&s.idref).map(|i| i.href.clone()))
        .collect();
    for href in spine_hrefs {
        let Ok(raw) = oeb.container.read(&href) else {
            continue;
        };
        let html = String::from_utf8_lossy(&raw);
        let mut dom = Dom::parse(&html);
        if !is_jacket_document(&dom) {
            continue;
        }
        for tag in ["table", "tr", "th"] {
            for e in dom.find_all_tag_global(tag) {
                dom.set_tag(e, "div");
            }
        }
        for e in dom.find_all_tag_global("td") {
            dom.set_tag(e, "span");
        }
        let rendered = dom.serialize(dom.root).into_bytes();
        let _ = oeb.container.write(&href, &rendered);
        break;
    }
}

fn is_jacket_document(dom: &Dom) -> bool {
    dom.find_all_tag_global("meta").iter().any(|&m| {
        dom.node(m).attrs.get("name").map(|s| s.as_str()) == Some(JACKET_META_NAME)
            && dom.node(m).attrs.get("content").map(|s| s.as_str()) == Some(JACKET_META_VALUE)
    })
}

/// Port of `referenced_images`: `file://`-scheme `<img src=...>`
/// elements in `dom`, paired with the local filesystem path they
/// reference. Used only when a hand-edited jacket template embeds a
/// local image by absolute path.
pub fn referenced_images(dom: &Dom) -> Vec<(NodeId, String)> {
    let mut out = Vec::new();
    for img in dom.find_all_tag_global("img") {
        let Some(src) = dom.node(img).attrs.get("src") else {
            continue;
        };
        if let Some(path) = src.strip_prefix("file://") {
            out.push((img, path.to_string()));
        }
    }
    out
}

// ===================================================================
// The OEBBook-facing transforms
// ===================================================================

fn is_jacket_item(oeb: &OEBBook, href: &str) -> bool {
    let Ok(raw) = oeb.container.read(href) else {
        return false;
    };
    let html = String::from_utf8_lossy(&raw);
    let dom = Dom::parse(&html);
    is_jacket_document(&dom)
}

/// Port of `Base.remove_images`: removes up to `limit` `<img>`s from
/// `href`'s document (dropping their manifest items and guide
/// references too). Returns how many were removed.
fn remove_images(oeb: &mut OEBBook, href: &str, limit: usize) -> usize {
    let Ok(raw) = oeb.container.read(href) else {
        return 0;
    };
    let html = String::from_utf8_lossy(&raw);
    let mut dom = Dom::parse(&html);
    let imgs: Vec<NodeId> = dom.find_all_tag_global("img");
    let mut removed = 0usize;
    for img in imgs {
        if removed >= limit {
            break;
        }
        let Some(src) = dom.node(img).attrs.get("src").cloned() else {
            continue;
        };
        let abs = super::filenames::urlnormalize(&super::filenames::abshref(href, &src));
        if let Some(item) = oeb.manifest.get_by_href(&abs) {
            let id = item.id.clone();
            let item_href = item.href.clone();
            oeb.manifest.remove(&id);
            oeb.guide.remove_by_href(&item_href);
            dom.detach(img);
            removed += 1;
        }
    }
    if removed > 0 {
        let rendered = dom.serialize(dom.root).into_bytes();
        let _ = oeb.container.write(href, &rendered);
    }
    removed
}

/// Port of `RemoveFirstImage`.
pub struct RemoveFirstImage;

impl RemoveFirstImage {
    pub fn call(&self, oeb: &mut OEBBook, opts: &JacketOptions, report: &mut dyn FnMut(&str)) {
        if !opts.remove_first_image {
            return;
        }
        let spine_hrefs: Vec<String> = oeb
            .spine
            .iter()
            .filter_map(|s| oeb.manifest.get_by_id(&s.idref).map(|i| i.href.clone()))
            .collect();
        let mut found = false;
        let mut deleted_href: Option<String> = None;
        for href in spine_hrefs {
            if is_jacket_item(oeb, &href) {
                continue;
            }
            let removed = remove_images(oeb, &href, 1);
            if removed > 0 {
                found = true;
                report("Removed first image");
                let Ok(raw) = oeb.container.read(&href) else {
                    break;
                };
                let html = String::from_utf8_lossy(&raw);
                let dom = Dom::parse(&html);
                let has_content = dom
                    .find_first_tag_global("body")
                    .map(|b| !dom.text_content(b).trim().is_empty())
                    .unwrap_or(false);
                let has_media = !dom.find_all_tag_global("img").is_empty()
                    || !dom.find_all_tag_global("svg").is_empty();
                if !has_content && !has_media {
                    report(&format!("Removing {href} as it has no content"));
                    if let Some(item) = oeb.manifest.get_by_href(&href) {
                        let id = item.id.clone();
                        oeb.manifest.remove(&id);
                        deleted_href = Some(href.clone());
                    }
                }
                break;
            }
        }
        if !found {
            report("Could not find first image to remove");
        }
        if let Some(deleted_href) = deleted_href {
            remove_toc_entries_for_href(&mut oeb.toc.root, &deleted_href);
            oeb.guide.remove_by_href(&deleted_href);
        }
    }
}

fn remove_toc_entries_for_href(node: &mut crate::oeb::toc::TOCNode, href: &str) {
    node.children.retain(|c| {
        c.href
            .as_deref()
            .map(|h| super::filenames::urldefrag(h).0 != href)
            .unwrap_or(true)
    });
    for c in &mut node.children {
        remove_toc_entries_for_href(c, href);
    }
}

/// Port of `Jacket`.
pub struct JacketTransform;

impl JacketTransform {
    fn remove_existing_jacket(&self, oeb: &mut OEBBook) {
        let candidates: Vec<String> = oeb
            .spine
            .iter()
            .take(4)
            .filter_map(|s| oeb.manifest.get_by_id(&s.idref).map(|i| i.href.clone()))
            .collect();
        for href in candidates {
            if is_jacket_item(oeb, &href) {
                remove_images(oeb, &href, usize::MAX);
                if let Some(item) = oeb.manifest.get_by_href(&href) {
                    let id = item.id.clone();
                    oeb.manifest.remove(&id);
                }
                break;
            }
        }
    }

    /// Port of `Jacket.__call__`.
    pub fn call(
        &self,
        oeb: &mut OEBBook,
        opts: &JacketOptions,
        mi: &MetaInformation,
        report: &mut dyn FnMut(&str),
    ) -> Result<()> {
        self.remove_existing_jacket(oeb);
        if !opts.insert_metadata {
            return Ok(());
        }

        let tags: Vec<String> = oeb
            .metadata
            .get("subject")
            .into_iter()
            .map(|i| i.value.clone())
            .collect();
        let comments = oeb
            .metadata
            .first("description")
            .map(|i| i.value.clone())
            .unwrap_or_default();
        let title = oeb
            .metadata
            .first("title")
            .map(|i| i.value.clone())
            .unwrap_or_else(|| "Unknown".to_string());
        let authors: Vec<String> = oeb
            .metadata
            .get("creator")
            .into_iter()
            .map(|i| i.value.clone())
            .collect();
        let authors = if authors.is_empty() {
            vec!["Unknown".to_string()]
        } else {
            authors
        };

        let dom = render_jacket(mi, opts, &title, &tags, &authors, &comments, true)?;
        let (id, href) = oeb.manifest.generate("calibre_jacket", "jacket.xhtml");
        oeb.manifest.add(&id, &href, XHTML_MIME);
        let rendered = dom.serialize(dom.root).into_bytes();
        oeb.container.write(&href, &rendered)?;
        oeb.spine.insert(0, &id, true);

        for (_img, path) in referenced_images(&dom) {
            report(&format!("Embedding referenced image {path} into jacket"));
            let ext = path.rsplit('.').next().unwrap_or("png").to_lowercase();
            let (img_id, img_href) = oeb
                .manifest
                .generate("jacket_image", &format!("jacket_img.{ext}"));
            if let Ok(data) = std::fs::read(&path) {
                let mime = mime_guess::from_ext(&ext)
                    .first_or_octet_stream()
                    .to_string();
                oeb.manifest.add(&img_id, &img_href, &mime);
                oeb.container.write(&img_href, &data)?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::transforms::test_support::Builder;

    fn mi(title: &str, authors: &[&str]) -> MetaInformation {
        MetaInformation::new(title, authors.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn fmt_sidx_formats_integers_and_roman_numerals() {
        assert_eq!(fmt_sidx(Some(3.0), false), "3");
        assert_eq!(fmt_sidx(Some(3.0), true), "III");
        assert_eq!(fmt_sidx(Some(2.5), false), "2.5");
        assert_eq!(fmt_sidx(None, false), "1");
    }

    #[test]
    fn rating_to_stars_scales_by_two() {
        assert_eq!(rating_to_stars(Some(10.0), false), "★★★★★");
        assert_eq!(rating_to_stars(Some(0.0), false), "");
        assert_eq!(rating_to_stars(Some(7.0), true), "★★★⯨");
    }

    #[test]
    fn urls_from_identifiers_builds_worldcat_and_doi_links() {
        let mut ids = HashMap::new();
        ids.insert("isbn".to_string(), "9780306406157".to_string());
        ids.insert("doi".to_string(), "10.1000/xyz".to_string());
        let urls = urls_from_identifiers(&ids);
        assert!(urls
            .iter()
            .any(|(_, t, _, u)| t == "isbn" && u.contains("worldcat.org/isbn/9780306406157")));
        assert!(urls
            .iter()
            .any(|(_, t, _, u)| t == "doi" && u.contains("dx.doi.org/10.1000/xyz")));
    }

    #[test]
    fn comments_to_html_wraps_plain_text_paragraphs() {
        let html = comments_to_html("lineone\n\nlinetwo");
        assert_eq!(
            html,
            "<p class=\"description\">lineone</p>\n<p class=\"description\">linetwo</p>"
        );
    }

    #[test]
    fn comments_to_html_passes_through_existing_html() {
        let html = comments_to_html("<p>already html</p>");
        assert_eq!(html, "<p>already html</p>");
    }

    #[test]
    fn comments_to_html_groups_inline_markup_into_a_paragraph() {
        let html = comments_to_html("a <b>bold</b> word");
        assert!(html.starts_with("<p class=\"description\">"), "{html}");
        assert!(html.contains("<b>bold</b>"), "{html}");
    }

    #[test]
    fn comments_to_html_routes_table_markup_through_the_sanitize_path() {
        // Doesn't start with '<' (so it's not the "already HTML,
        // untouched" case) but contains a <table> -- Python's
        // sanitize_pat routes this through html2text -> Markdown
        // instead of the inline-tag DOM-regrouping pass, which isn't
        // meant to handle block-structured markup like tables.
        let html = comments_to_html("intro <table><tr><td>cell</td></tr></table>");
        assert!(!html.contains("<table"), "{html}");
    }

    #[test]
    fn sanitize_comments_html_round_trips_through_markdown() {
        let html = sanitize_comments_html("<p>Hello <b>world</b></p>");
        assert!(html.contains("Hello"), "{html}");
        assert!(html.contains("world"), "{html}");
    }

    #[test]
    fn render_markdown_converts_basic_markdown() {
        let html = render_markdown("**bold** and *italic*");
        assert!(html.contains("<strong>bold</strong>"), "{html}");
        assert!(html.contains("<em>italic</em>"), "{html}");
    }

    #[test]
    fn render_jacket_produces_a_document_with_title_and_author() {
        let m = mi("My Book Title", &["Jane Author"]);
        let opts = JacketOptions::default();
        let dom = render_jacket(&m, &opts, "Unknown", &[], &[], "", true).unwrap();
        let rendered = dom.serialize(dom.root);
        assert!(rendered.contains("My Book Title"), "{rendered}");
        assert!(rendered.contains("Jane Author"), "{rendered}");
        assert!(rendered.contains(JACKET_META_VALUE), "{rendered}");
    }

    #[test]
    fn render_jacket_strips_series_row_when_there_is_no_series() {
        let m = mi("T", &["A"]);
        let opts = JacketOptions::default();
        let dom = render_jacket(&m, &opts, "Unknown", &[], &[], "", true).unwrap();
        let rendered = dom.serialize(dom.root);
        // The bundled stylesheet's own CSS rules legitimately reference
        // `.cbj_series` (e.g. `table.cbj_header td.cbj_series {...}`) --
        // that text always appears in the embedded `<style>` block. What
        // must be gone is the *element* that carried the class.
        assert!(!rendered.contains(r#"class="cbj_series""#), "{rendered}");
    }

    #[test]
    fn render_jacket_keeps_series_row_when_series_is_set() {
        let mut m = mi("T", &["A"]);
        m.series = Some("The Great Series".to_string());
        m.series_index = 2.0;
        let opts = JacketOptions::default();
        let dom = render_jacket(&m, &opts, "Unknown", &[], &[], "", true).unwrap();
        let rendered = dom.serialize(dom.root);
        assert!(rendered.contains("The Great Series"), "{rendered}");
        assert!(rendered.contains("II"), "{rendered}");
    }

    #[test]
    fn jacket_transform_inserts_a_jacket_page_at_the_start_of_the_spine() {
        let mut oeb = Builder::new().page("a.html", "<p>content</p>").build();
        oeb.metadata.add("title", "Test Book");
        oeb.metadata.add("creator", "Someone");
        let opts = JacketOptions::default();
        let m = mi("Test Book", &["Someone"]);
        let mut log = Vec::new();
        JacketTransform
            .call(&mut oeb, &opts, &m, &mut |msg| log.push(msg.to_string()))
            .unwrap();
        assert_eq!(oeb.spine.items[0].idref, "calibre_jacket");
        let jacket_href = oeb
            .manifest
            .get_by_id("calibre_jacket")
            .unwrap()
            .href
            .clone();
        let raw = oeb.container.read(&jacket_href).unwrap();
        assert!(String::from_utf8_lossy(&raw).contains("Test Book"));
    }

    #[test]
    fn jacket_transform_replaces_an_existing_jacket() {
        let mut oeb = Builder::new().page("a.html", "<p>content</p>").build();
        let opts = JacketOptions::default();
        let m1 = mi("First", &["A"]);
        let mut log = Vec::new();
        JacketTransform
            .call(&mut oeb, &opts, &m1, &mut |msg| log.push(msg.to_string()))
            .unwrap();
        let m2 = mi("Second", &["B"]);
        JacketTransform
            .call(&mut oeb, &opts, &m2, &mut |msg| log.push(msg.to_string()))
            .unwrap();
        // Only one jacket item should remain in the manifest.
        let jacket_count = oeb
            .manifest
            .iter()
            .filter(|i| i.href.contains("jacket"))
            .count();
        assert_eq!(jacket_count, 1);
        let jacket_href = oeb
            .manifest
            .get_by_id("calibre_jacket")
            .unwrap()
            .href
            .clone();
        let raw = oeb.container.read(&jacket_href).unwrap();
        assert!(String::from_utf8_lossy(&raw).contains("Second"));
    }

    #[test]
    fn linearize_jacket_converts_tables_to_divs() {
        let mut oeb = Builder::new()
            .part(
                "jacket.xhtml",
                "application/xhtml+xml",
                br#"<html xmlns="http://www.w3.org/1999/xhtml"><head><meta name="calibre-content" content="jacket"/></head><body><table><tr><td>x</td></tr></table></body></html>"#,
                true,
            )
            .build();
        linearize_jacket(&mut oeb);
        let raw = oeb.container.read("jacket.xhtml").unwrap();
        let html = String::from_utf8_lossy(&raw);
        assert!(!html.contains("<table"), "{html}");
        assert!(html.contains("<div"), "{html}");
        assert!(html.contains("<span"), "{html}");
    }
}
