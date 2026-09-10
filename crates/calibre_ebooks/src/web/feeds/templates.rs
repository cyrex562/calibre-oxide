//! Port of `old_src/src/calibre/web/feeds/templates.py` (issue #618,
//! split from #81): the 8 index/feed/navbar HTML template classes
//! that lay out downloaded news content.
//!
//! # A tiny element builder, not a full DOM
//!
//! Real Python builds each page via `lxml.html.builder` (`DIV(...)`,
//! `A(...)`, etc.) then serializes with `lxml.etree`. This module uses
//! a small internal [`El`]/[`Node`] tree (element-or-text, matching
//! lxml's own text/tail model one-for-one: lxml's "set an element's
//! `.tail` to insert text right after it" idiom is replicated here by
//! simply pushing a `Node::Text` sibling immediately after that
//! element, which renders identically and needs no special-cased
//! "tail" concept). **Disclosed narrowing**: real Python's `IS_HTML`
//! flag switches between `lxml.html.tostring` (HTML void-element
//! syntax, `<br>`) and `lxml.etree.tostring(xml_declaration=True)`
//! (XML self-closing syntax, `<br/>`) for 3 of the 8 templates
//! (`IndexTemplate`/`TouchscreenIndexTemplate`/`TouchscreenFeedTemplate`).
//! This port always serializes as HTML (with self-closing void tags,
//! which is valid in both HTML5 and XHTML) -- the structural content
//! is identical either way; only the literal serialization mode
//! differs, and no downstream real caller in this port's scope
//! inspects that. Real Python's `pretty_print=True` whitespace
//! formatting is also not replicated (cosmetic only).
//!
//! # A real, confirmed dead-code branch, not reproduced
//!
//! `FeedTemplate`/`TouchscreenFeedTemplate` both guard a feed-image
//! `<img>` block on `getattr(feed, 'image', None)` -- but real
//! Python's own `Feed.__init__` (see `web::feeds`'s own module doc)
//! never sets a `self.image` attribute anywhere; only `image_url`/
//! `image_width`/`image_height`/`image_alt` exist. `getattr(feed,
//! 'image', None)` is therefore always `None` in every real feed --
//! this branch can never execute in upstream itself. Confirmed by
//! reading `__init__.py` directly, not assumed; this port omits the
//! dead branch entirely rather than „supporting" a `Feed.image` field
//! that doesn't exist and never did.

use crate::web::feeds::{Article, Feed};

const VOID_TAGS: &[&str] = &["br", "hr", "img"];

#[derive(Debug, Clone)]
enum Node {
    Text(String),
    /// Pre-serialized HTML emitted verbatim, not escaped -- used only
    /// by [`generate_embedded_content`] to reparent an article's own
    /// already-real HTML content, matching real Python's own
    /// "reparent the parsed fragment's nodes unchanged" behavior.
    Raw(String),
    El(El),
}

#[derive(Debug, Clone)]
struct El {
    tag: &'static str,
    attrs: Vec<(&'static str, String)>,
    children: Vec<Node>,
}

impl El {
    fn new(tag: &'static str) -> Self {
        El { tag, attrs: Vec::new(), children: Vec::new() }
    }

    fn attr(mut self, k: &'static str, v: impl Into<String>) -> Self {
        self.attrs.push((k, v.into()));
        self
    }

    fn maybe_attr(self, k: &'static str, v: Option<impl Into<String>>) -> Self {
        match v {
            Some(v) => self.attr(k, v),
            None => self,
        }
    }

    fn class(self, classes: &[&str]) -> Self {
        if classes.is_empty() {
            self
        } else {
            self.attr("class", classes.join(" "))
        }
    }

    fn rescale(self, pct: u32) -> Self {
        self.attr("data-calibre-rescale", pct.to_string())
    }

    fn push(mut self, n: impl Into<Node>) -> Self {
        self.children.push(n.into());
        self
    }

    fn push_opt(self, n: Option<impl Into<Node>>) -> Self {
        match n {
            Some(n) => self.push(n),
            None => self,
        }
    }

    fn text(self, t: impl Into<String>) -> Self {
        self.push(Node::Text(t.into()))
    }
}

impl From<El> for Node {
    fn from(e: El) -> Node {
        Node::El(e)
    }
}
impl From<String> for Node {
    fn from(s: String) -> Node {
        Node::Text(s)
    }
}
impl From<&str> for Node {
    fn from(s: &str) -> Node {
        Node::Text(s.to_string())
    }
}

fn escape_text(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn escape_attr(s: &str) -> String {
    escape_text(s).replace('"', "&quot;")
}

fn render_node(node: &Node, out: &mut String) {
    match node {
        Node::Text(t) => out.push_str(&escape_text(t)),
        Node::Raw(t) => out.push_str(t),
        Node::El(e) => {
            out.push('<');
            out.push_str(e.tag);
            for (k, v) in &e.attrs {
                out.push(' ');
                out.push_str(k);
                out.push_str("=\"");
                out.push_str(&escape_attr(v));
                out.push('"');
            }
            if e.children.is_empty() && VOID_TAGS.contains(&e.tag) {
                out.push_str(" />");
                return;
            }
            out.push('>');
            for c in &e.children {
                render_node(c, out);
            }
            out.push_str("</");
            out.push_str(e.tag);
            out.push('>');
        }
    }
}

/// Port of `Template.render`: serializes a built page (`HEAD`+`BODY`
/// root) to a real HTML string, including the real
/// `include_meta_content_type=True` charset meta tag.
fn render_page(mut head: El, body: El, html_lang: Option<&str>) -> String {
    head.children.insert(0, Node::El(El::new("meta").attr("http-equiv", "Content-Type").attr("content", "text/html; charset=utf-8")));
    let mut html_el = El::new("html").push(head).push(body);
    if let Some(lang) = html_lang {
        html_el = html_el.attr("lang", lang);
    }
    let mut out = String::from("<!DOCTYPE html>\n");
    render_node(&Node::El(html_el), &mut out);
    out
}

fn head_el(title: &str, style: Option<&str>, extra_css: Option<&str>) -> El {
    let mut head = El::new("head").push(El::new("title").text(title.to_string()));
    if let Some(style) = style {
        head = head.push(El::new("style").attr("type", "text/css").text(style.to_string()));
    }
    if let Some(extra_css) = extra_css {
        head = head.push(El::new("style").attr("type", "text/css").text(extra_css.to_string()));
    }
    head
}

// ===================================================================
// EmbeddedContent
// ===================================================================

/// Port of `EmbeddedContent._generate`. Real Python parses `text`
/// (the article's own content/summary, whichever is longer) as an
/// HTML fragment and re-hosts its top-level nodes inside a fresh
/// `<div>`. This port takes the already-real HTML string and embeds
/// it verbatim as the div's raw inner HTML (real Python does the
/// equivalent -- reparenting the parsed fragment's own nodes
/// unchanged -- just via a different, tree-walking mechanism); no
/// text/tail juggling is needed since the fragment is inserted whole,
/// not rebuilt node-by-node.
pub fn generate_embedded_content(article: &Article, style: Option<&str>, extra_css: Option<&str>) -> String {
    let content = article.content.as_deref().unwrap_or("");
    let summary = article.summary.as_deref().unwrap_or("");
    let text = if content.len() > summary.len() { content } else { summary };

    let head = head_el(&article.title, style, extra_css);
    let div = El::new("div").push(Node::Raw(text.to_string()));
    let body = El::new("body").push(El::new("h2").text(article.title.clone())).push(div);
    render_page(head, body, None)
}

// ===================================================================
// IndexTemplate
// ===================================================================

/// Port of `IndexTemplate._generate`.
#[allow(clippy::too_many_arguments)]
pub fn generate_index(title: &str, masthead: &str, date_str: &str, feeds: &[Feed], extra_css: Option<&str>, style: Option<&str>, html_lang: Option<&str>) -> String {
    let head = head_el(title, style, extra_css);
    let mut ul = El::new("ul").class(&["calibre_feed_list"]);
    for (i, feed) in feeds.iter().enumerate() {
        if !feed.is_empty() {
            let a = El::new("a").class(&["feed"]).rescale(120).attr("href", format!("feed_{i}/index.html")).text(feed.title.clone());
            ul = ul.push(El::new("li").attr("id", format!("feed_{i}")).push(a));
        }
    }
    let div = El::new("div")
        .rescale(100)
        .push(El::new("p").attr("style", "text-align:center").push(El::new("img").attr("src", masthead).attr("alt", "masthead")))
        .push(El::new("p").attr("style", "text-align:right").text(date_str.to_string()))
        .push(ul);
    let body = El::new("body").push(div);
    render_page(head, body, html_lang)
}

// ===================================================================
// FeedTemplate
// ===================================================================

/// Port of `FeedTemplate.get_navbar`.
fn feed_navbar(f: usize, feeds: &[Feed], top: bool) -> El {
    if feeds.len() < 2 {
        return El::new("div");
    }
    let mut navbar = El::new("div").class(&["calibre_navbar"]).rescale(70).attr("style", "text-align:center");
    if !top {
        navbar = navbar.push(El::new("hr"));
    } else {
        navbar = navbar.text("| ");
    }
    if f + 1 < feeds.len() {
        navbar = navbar.push(El::new("a").attr("href", format!("../feed_{}/index.html", f + 1)).text("Next section")).text(" | ");
    }
    navbar = navbar.push(El::new("a").attr("href", "../index.html").text("Main menu")).text(" | ");
    if f > 0 {
        navbar = navbar.push(El::new("a").attr("href", format!("../feed_{}/index.html", f - 1)).text("Previous section")).text(" |");
    }
    if top {
        navbar = navbar.push(El::new("hr"));
    }
    navbar
}

/// Port of `FeedTemplate._generate`. `cutoff` replaces real Python's
/// `description_limiter` (a `BasicNewsRecipe` method, issue #619+, not
/// yet ported) as an injected closure.
pub fn generate_feed(f: usize, feeds: &[Feed], cutoff: impl Fn(&str) -> String, extra_css: Option<&str>, style: Option<&str>, html_lang: Option<&str>) -> String {
    let feed = &feeds[f];
    let head = head_el(&feed.title, style, extra_css);
    let mut body = El::new("body").push(feed_navbar(f, feeds, true));

    let mut div = El::new("div").rescale(100).push(El::new("h2").class(&["calibre_feed_title"]).rescale(160).text(feed.title.clone()));
    // Real Python's `getattr(feed, 'image', None)` branch is dead code
    // -- see this module's own doc -- omitted here, not ported.
    if !feed.description.is_empty() {
        let cleaned = calibre_utils::cleantext::clean_xml_chars(&feed.description);
        div = div.push(El::new("div").class(&["calibre_feed_description"]).rescale(80).text(cleaned).push(El::new("br")));
    }
    let mut ul = El::new("ul").class(&["calibre_article_list"]);
    for (i, article) in feed.articles.iter().enumerate() {
        if !article.downloaded {
            continue;
        }
        let mut li = El::new("li")
            .rescale(100)
            .attr("id", format!("article_{i}"))
            .attr("style", "padding-bottom:0.5em")
            .push(El::new("a").class(&["article"]).rescale(120).maybe_attr("href", article.url.clone()).text(article.title.clone()))
            .push(El::new("span").class(&["article_date"]).text(article.formatted_date()));
        if article.summary.is_some() {
            let cleaned = calibre_utils::cleantext::clean_xml_chars(&cutoff(&article.text_summary));
            li = li.push(El::new("div").class(&["article_description"]).rescale(70).text(cleaned));
        }
        ul = ul.push(li);
    }
    div = div.push(ul).push(feed_navbar(f, feeds, false));
    body = body.push(div);
    render_page(head, body, html_lang)
}

// ===================================================================
// NavBarTemplate
// ===================================================================

/// Port of `NavBarTemplate._generate`. `two_levels` is accepted for
/// real-signature parity but, like real Python, never actually used
/// in the body (confirmed by reading the real source -- a genuine
/// unused parameter upstream itself carries).
#[allow(clippy::too_many_arguments, unused_variables)]
pub fn generate_navbar(bottom: bool, feed: usize, art: usize, number_of_articles_in_feed: usize, two_levels: bool, url: &str, app_name: &str, prefix: &str, center: bool, extra_css: Option<&str>, style: Option<&str>) -> String {
    let head = head_el("navbar", style, extra_css);
    let prefix = if !prefix.is_empty() && !prefix.ends_with('/') { format!("{prefix}/") } else { prefix.to_string() };
    let align = if center { "center" } else { "left" };

    let mut navbar = El::new("div").class(&["calibre_navbar"]).rescale(70).attr("style", format!("text-align:{align}"));
    if bottom {
        if !url.starts_with("file://") {
            navbar = navbar.push(El::new("hr"));
            let p = El::new("p")
                .attr("style", "text-align:left; max-width: 100%; overflow: hidden;")
                .text("This article was downloaded by ")
                .push(El::new("strong").text(app_name.to_string()))
                .text(" from ")
                .push(El::new("a").attr("href", url).attr("rel", "calibre-downloaded-from").text(url.to_string()));
            navbar = navbar.push(p).push(El::new("br"));
        }
        navbar = navbar.push(El::new("br"));
    } else {
        let next_art = if art == number_of_articles_in_feed - 1 { format!("feed_{}", feed + 1) } else { format!("article_{}", art + 1) };
        let up = if art == number_of_articles_in_feed - 1 { "../.." } else { ".." };
        let href = format!("{prefix}{up}/{next_art}/index.html");
        navbar = navbar.text("| ").push(El::new("a").attr("href", href).attr("rel", "articlenextlink").text("Next"));
    }
    let href = format!("{prefix}../index.html#article_{art}");
    navbar = navbar.text(" | ").push(El::new("a").attr("href", href).text("Section menu"));
    let href = format!("{prefix}../../index.html#feed_{feed}");
    navbar = navbar.text(" | ").push(El::new("a").attr("href", href).text("Main menu"));
    if art > 0 && !bottom {
        let href = format!("{prefix}../article_{}/index.html", art - 1);
        navbar = navbar.text(" | ").push(El::new("a").attr("href", href).attr("rel", "articleprevlink").text("Previous"));
    }
    navbar = navbar.text(" | ");
    if !bottom {
        navbar = navbar.push(El::new("hr"));
    }

    render_page(head, El::new("body").push(navbar), None)
}

// ===================================================================
// Touchscreen templates
// ===================================================================

/// Port of `TouchscreenIndexTemplate._generate`. `date_str` replaces
/// real Python's own `'{}, {} {}, {}'.format(strftime('%A'), ...)`
/// locale-dependent assembly -- callers pass an already-formatted
/// string (this crate has no `strftime`-locale layer to replicate
/// that construction faithfully; formatting the date is the caller's
/// job, same disclosed shape as [`generate_index`]'s `date_str`).
pub fn generate_touchscreen_index(title: &str, masthead: &str, date_str: &str, feeds: &[Feed], extra_css: Option<&str>, style: Option<&str>, html_lang: Option<&str>) -> String {
    let head = head_el(title, style, extra_css);
    let masthead_p = El::new("p").attr("style", "text-align:center").push(El::new("img").attr("src", masthead).attr("alt", "masthead"));

    let mut toc = El::new("table").class(&["toc"]).attr("width", "100%").attr("border", "0").attr("cellpadding", "3px");
    for (i, feed) in feeds.iter().enumerate() {
        if !feed.is_empty() {
            let tr = El::new("tr")
                .push(El::new("td").rescale(120).push(El::new("a").attr("href", format!("feed_{i}/index.html")).text(feed.title.clone())))
                .push(El::new("td").attr("style", "text-align:right").text(feed.articles.len().to_string()));
            toc = toc.push(tr);
        }
    }
    let div = El::new("div").push(masthead_p).push(El::new("h3").class(&["publish_date"]).text(date_str.to_string())).push(El::new("div").class(&["divider"])).push(toc);
    render_page(head, El::new("body").push(div), html_lang)
}

/// Port of `TouchscreenFeedTemplate::trim_title`.
fn trim_title(title: &str, clip: usize) -> String {
    if title.chars().count() <= clip {
        return title.to_string();
    }
    let tokens: Vec<&str> = title.split(' ').collect();
    if tokens.first().map(|t| t.chars().count()).unwrap_or(0) > clip {
        return format!("{}...", tokens[0].chars().take(clip).collect::<String>());
    }
    let mut new_tokens = Vec::new();
    let mut len = 0usize;
    for token in &tokens {
        if token.chars().count() + len < clip {
            new_tokens.push(*token);
            len += token.chars().count();
        } else {
            new_tokens.push("...");
            return new_tokens.join(" ");
        }
    }
    title.to_string()
}

/// Port of `TouchscreenFeedTemplate._generate`.
pub fn generate_touchscreen_feed(f: usize, feeds: &[Feed], cutoff: impl Fn(&str) -> String, extra_css: Option<&str>, style: Option<&str>, html_lang: Option<&str>) -> String {
    let feed = &feeds[f];

    let build_navbar = || {
        let prev_link = if f > 0 { Some(El::new("a").class(&["feed_link"]).attr("href", format!("../feed_{}/index.html", f - 1)).text(trim_title(&feeds[f - 1].title, 18))) } else { None };
        let next_link = if f < feeds.len() - 1 { Some(El::new("a").class(&["feed_link"]).attr("href", format!("../feed_{}/index.html", f + 1)).text(trim_title(&feeds[f + 1].title, 18))) } else { None };
        let tr = El::new("tr")
            .push(El::new("td").class(&["feed_prev"]).push_opt(prev_link))
            .push(El::new("td").class(&["feed_up"]).push(El::new("a").attr("href", "../index.html").text("Sections")))
            .push(El::new("td").class(&["feed_next"]).push_opt(next_link));
        El::new("table").class(&["touchscreen_navbar"]).push(tr)
    };

    let head = head_el(&feed.title, style, extra_css);
    let mut div = El::new("div").push(build_navbar()).push(El::new("h2").class(&["feed_title"]).text(feed.title.clone()));
    // Real Python's `getattr(feed, 'image', None)` branch is dead code
    // -- see this module's own doc -- omitted here, not ported.
    if !feed.description.is_empty() {
        let cleaned = calibre_utils::cleantext::clean_xml_chars(&feed.description);
        div = div.push(El::new("div").class(&["calibre_feed_description"]).rescale(80).text(cleaned).push(El::new("br")));
    }
    for article in feed.articles.iter() {
        if !article.downloaded {
            continue;
        }
        let mut a = El::new("div").class(&["article_summary"]).push(El::new("a").class(&["summary_headline"]).rescale(120).maybe_attr("href", article.url.clone()).text(article.title.clone()));
        if let Some(author) = &article.author {
            a = a.push(El::new("div").class(&["summary_byline"]).rescale(100).text(author.clone()));
        }
        if article.summary.is_some() {
            a = a.push(El::new("div").class(&["summary_text"]).rescale(100).text(cutoff(&article.text_summary)));
        }
        div = div.push(a);
    }
    div = div.push(build_navbar());
    render_page(head, El::new("body").push(div), html_lang)
}

/// Port of `TouchscreenNavBarTemplate._generate`.
#[allow(clippy::too_many_arguments)]
pub fn generate_touchscreen_navbar(bottom: bool, feed: usize, art: usize, number_of_articles_in_feed: usize, url: &str, app_name: &str, prefix: &str, extra_css: Option<&str>, style: Option<&str>) -> String {
    let head = head_el("navbar", style, extra_css);
    let prefix = if !prefix.is_empty() && !prefix.ends_with('/') { format!("{prefix}/") } else { prefix.to_string() };

    let mut navbar = El::new("div");
    if bottom && !url.starts_with("file://") {
        let p = El::new("p")
            .attr("style", "text-align:left; max-width: 100%; overflow: hidden;")
            .text("This article was downloaded by ")
            .push(El::new("strong").text(app_name.to_string()))
            .text(" from ")
            .push(El::new("a").attr("href", url).attr("rel", "calibre-downloaded-from").text(url.to_string()));
        navbar = navbar.push(El::new("hr")).push(p).push(El::new("br"));
    }

    let mut tr = El::new("tr");
    if art > 0 {
        let href = format!("{prefix}../article_{}/index.html", art - 1);
        tr = tr.push(El::new("td").class(&["article_prev"]).push(El::new("a").class(&["article_link"]).attr("rel", "articleprevlink").attr("href", href).text("Previous")));
    } else {
        tr = tr.push(El::new("td").class(&["article_prev"]));
    }
    tr = tr.push(El::new("td").class(&["article_articles_list"]).push(El::new("a").class(&["articles_link"]).attr("href", format!("{prefix}../index.html#article_{art}")).text("Articles")));
    tr = tr.push(El::new("td").class(&["article_sections_list"]).push(El::new("a").class(&["sections_link"]).attr("href", format!("{prefix}../../index.html#feed_{feed}")).text("Sections")));

    let next_art = if art == number_of_articles_in_feed - 1 { format!("feed_{}", feed + 1) } else { format!("article_{}", art + 1) };
    let up = if art == number_of_articles_in_feed - 1 { "../.." } else { ".." };
    tr = tr.push(El::new("td").class(&["article_next"]).push(El::new("a").class(&["article_link"]).attr("rel", "articlenextlink").attr("href", format!("{prefix}{up}/{next_art}/index.html")).text("Next")));

    navbar = navbar.push(El::new("table").class(&["touchscreen_navbar"]).push(tr));
    render_page(head, El::new("body").push(navbar), None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::feeds::Article;
    use chrono::Utc;

    fn feed_with(title: &str, articles: Vec<Article>) -> Feed {
        let mut f = crate::web::feeds::Feed::populate_from_preparsed_feed(Some(title), &[], 36500.0, 100);
        f.articles = articles;
        f
    }

    fn downloaded_article(title: &str, url: &str) -> Article {
        let mut a = Article::new("1", Some(title), Some(url.to_string()), None, Some("A summary".to_string()), Some(Utc::now()), None);
        a.downloaded = true;
        a
    }

    #[test]
    fn generate_index_lists_only_non_empty_feeds_with_correct_links() {
        let feeds = vec![feed_with("A", vec![downloaded_article("a1", "http://x/")]), feed_with("Empty", vec![]), feed_with("B", vec![downloaded_article("b1", "http://y/")])];
        let html = generate_index("Title", "masthead.jpg", "Mon, 01 Jan", &feeds, None, None, None);
        assert!(html.contains("<title>Title</title>"));
        assert!(html.contains(r#"href="feed_0/index.html""#));
        assert!(!html.contains("Empty")); // the empty feed is skipped entirely
        assert!(html.contains(r#"href="feed_2/index.html""#)); // real index preserved even though feed 1 was skipped
        assert!(html.contains(">A<"));
        assert!(html.contains(">B<"));
    }

    #[test]
    fn generate_feed_skips_non_downloaded_articles() {
        let mut not_downloaded = downloaded_article("skip me", "http://skip/");
        not_downloaded.downloaded = false;
        let feeds = vec![feed_with("Feed", vec![downloaded_article("keep me", "http://keep/"), not_downloaded])];
        let html = generate_feed(0, &feeds, |s| s.to_string(), None, None, None);
        assert!(html.contains("keep me"));
        assert!(!html.contains("skip me"));
    }

    #[test]
    fn generate_feed_applies_cutoff_to_the_summary() {
        let feeds = vec![feed_with("Feed", vec![downloaded_article("Art", "http://x/")])];
        let html = generate_feed(0, &feeds, |_s| "TRUNCATED".to_string(), None, None, None);
        assert!(html.contains("TRUNCATED"));
    }

    #[test]
    fn generate_feed_navbar_has_prev_and_next_links_for_a_middle_feed() {
        let feeds = vec![feed_with("A", vec![downloaded_article("a", "http://a/")]), feed_with("B", vec![downloaded_article("b", "http://b/")]), feed_with("C", vec![downloaded_article("c", "http://c/")])];
        let html = generate_feed(1, &feeds, |s| s.to_string(), None, None, None);
        assert!(html.contains("../feed_0/index.html"));
        assert!(html.contains("../feed_2/index.html"));
        assert!(html.contains("Previous section"));
        assert!(html.contains("Next section"));
    }

    #[test]
    fn generate_navbar_bottom_with_a_real_url_includes_the_downloaded_by_notice() {
        let html = generate_navbar(true, 0, 0, 3, false, "http://example.com/a", "calibre", "", true, None, None);
        assert!(html.contains("This article was downloaded by"));
        assert!(html.contains("<strong>calibre</strong>"));
        assert!(html.contains(r#"href="http://example.com/a""#));
    }

    #[test]
    fn generate_navbar_bottom_with_a_file_url_omits_the_downloaded_by_notice() {
        let html = generate_navbar(true, 0, 0, 3, false, "file:///tmp/x.html", "calibre", "", true, None, None);
        assert!(!html.contains("This article was downloaded by"));
    }

    #[test]
    fn generate_navbar_top_links_to_the_next_article_within_the_feed() {
        let html = generate_navbar(false, 2, 0, 3, false, "http://x/", "calibre", "", true, None, None);
        assert!(html.contains(r#"href="../article_1/index.html""#));
        assert!(html.contains("articlenextlink"));
    }

    #[test]
    fn generate_navbar_top_on_the_last_article_links_to_the_next_feed() {
        let html = generate_navbar(false, 2, 2, 3, false, "http://x/", "calibre", "", true, None, None);
        assert!(html.contains(r#"href="../../feed_3/index.html""#));
    }

    #[test]
    fn generate_navbar_omits_previous_link_for_the_first_article() {
        let html = generate_navbar(false, 0, 0, 3, false, "http://x/", "calibre", "", true, None, None);
        assert!(!html.contains("articleprevlink"));
    }

    #[test]
    fn trim_title_leaves_short_titles_unchanged() {
        assert_eq!(trim_title("Short", 18), "Short");
    }

    #[test]
    fn trim_title_truncates_a_long_single_word() {
        let long_word = "Supercalifragilisticexpialidocious";
        assert_eq!(trim_title(long_word, 18), format!("{}...", &long_word[..18]));
    }

    #[test]
    fn trim_title_truncates_at_a_word_boundary_for_multi_word_titles() {
        let title = "one two three four five six seven";
        let trimmed = trim_title(title, 18);
        assert!(trimmed.ends_with("..."));
        assert!(trimmed.len() <= title.len());
    }

    #[test]
    fn generate_touchscreen_index_lists_article_counts() {
        let feeds = vec![feed_with("A", vec![downloaded_article("a1", "http://x/"), downloaded_article("a2", "http://y/")])];
        let html = generate_touchscreen_index("T", "m.jpg", "Monday", &feeds, None, None, None);
        assert!(html.contains(">2<"));
    }

    #[test]
    fn generate_touchscreen_feed_includes_author_when_present() {
        let mut a = downloaded_article("Art", "http://x/");
        a.author = Some("Jane Doe".to_string());
        let feeds = vec![feed_with("Feed", vec![a])];
        let html = generate_touchscreen_feed(0, &feeds, |s| s.to_string(), None, None, None);
        assert!(html.contains("Jane Doe"));
    }

    #[test]
    fn generate_embedded_content_prefers_the_longer_of_content_and_summary() {
        let mut a = Article::new("1", Some("T"), None, None, Some("short".to_string()), None, None);
        a.content = Some("a much longer piece of content here".to_string());
        let html = generate_embedded_content(&a, None, None);
        assert!(html.contains("a much longer piece of content here"));
    }

    #[test]
    fn generate_touchscreen_navbar_shows_previous_only_past_the_first_article() {
        let html0 = generate_touchscreen_navbar(false, 0, 0, 3, "http://x/", "calibre", "", None, None);
        assert!(!html0.contains("articleprevlink"));
        let html1 = generate_touchscreen_navbar(false, 0, 1, 3, "http://x/", "calibre", "", None, None);
        assert!(html1.contains("articleprevlink"));
    }

    #[test]
    fn render_page_includes_the_real_charset_meta_tag() {
        let html = generate_index("T", "m.jpg", "d", &[], None, None, None);
        assert!(html.contains(r#"<meta http-equiv="Content-Type" content="text/html; charset=utf-8""#));
    }

    #[test]
    fn text_content_is_xml_escaped() {
        let feeds = vec![feed_with("A & B <tag>", vec![])];
        let html = generate_index("T", "m.jpg", "d", &feeds, None, None, None);
        // The feed itself is empty so it won't be listed, but title
        // escaping applies generally -- exercise it via the page title.
        let html2 = generate_index("Title & <script>", "m.jpg", "d", &feeds, None, None, None);
        assert!(html2.contains("Title &amp; &lt;script&gt;"));
        assert!(!html2.contains("<script>"));
    }
}
