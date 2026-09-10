//! Port of `old_src/src/calibre/web/feeds/__init__.py` (issue #617,
//! split from #81): [`Article`]/[`Feed`]/[`FeedCollection`] plus real
//! RSS/Atom feed parsing.
//!
//! # Feed parsing: `feed-rs`, not upstream's vendored `feedparser`
//!
//! Real Python's `feed_from_xml` parses raw feed bytes via
//! `calibre.web.feeds.feedparser.parse` -- a vendored third-party
//! library, not part of this porting corpus at all (it isn't in
//! `old_src`'s own module list, doesn't have a `modules_to_port.md`
//! entry, and is treated by upstream itself as an external runtime
//! dependency, not calibre's own code). This port needs a real
//! Rust feed parser instead; [`feed_rs`] (verified to resolve and
//! build cleanly) is that real, chosen replacement.
//!
//! # Real structural simplifications this brings, disclosed
//!
//! - `feed-rs` already resolves an entry's id (falling back to a hash
//!   of the first link, or a UUID) and already parses every date field
//!   into a real `DateTime<Utc>` -- real Python's own
//!   `date_parsed`/`published_parsed`/`updated_parsed` then
//!   `dateutil.parser.parse(date_field)` fallback chain exists purely
//!   to cover what `feedparser`'s own weaker date handling misses;
//!   `feed-rs` already does this work, so there is no analogous
//!   string-reparsing fallback to port here.
//! - Real Python's `content` field can hold *multiple* `<content:*>`
//!   elements (feedparser exposes it as a list, joined with `\n`);
//!   `feed-rs`'s [`feed_rs::model::Entry::content`] normalizes this to
//!   a single [`feed_rs::model::Content`]. [`Feed::parse_article`]
//!   uses that single body directly.

pub mod postprocess;
pub mod recipe;
pub mod templates;

use std::collections::HashSet;

use chrono::{DateTime, Utc};

use calibre_utils::cleantext::{clean_ascii_chars, clean_xml_chars};

use crate::dom::Dom;
use crate::html_entities::decode_entities;

/// Port of `Article`.
#[derive(Debug, Clone)]
pub struct Article {
    pub id: String,
    pub title: String,
    pub url: Option<String>,
    pub author: Option<String>,
    /// The raw (possibly HTML) summary, XML-char-cleaned.
    pub summary: Option<String>,
    /// HTML-stripped, ASCII-cleaned plain text of `summary`.
    pub text_summary: String,
    pub content: Option<String>,
    /// Port of `self.utctime`. Real Python also exposes a separate
    /// `self.localtime` (`self.utctime.astimezone(local_tz)`) --
    /// this port has no local-timezone-resolution equivalent ported
    /// yet anywhere in `calibre_utils::date`, so [`Article::formatted_date`]
    /// formats in UTC, a disclosed, narrow narrowing (display-only;
    /// every real comparison in this module already worked in UTC).
    pub date: DateTime<Utc>,
    pub toc_thumbnail: Option<String>,
    pub internal_toc_entries: Vec<String>,
    pub downloaded: bool,
}

impl Article {
    /// Port of `Article.__init__`.
    pub fn new(id: impl Into<String>, title: Option<&str>, url: Option<String>, author: Option<String>, summary: Option<String>, published: Option<DateTime<Utc>>, content: Option<String>) -> Self {
        let raw_title = match title {
            Some(t) if !t.is_empty() => t.to_string(),
            _ => "Unknown".to_string(),
        };
        let mut title = clean_xml_chars(&raw_title).trim().to_string();
        title = decode_entities(&title);
        title = clean_ascii_chars(&title);

        let summary = summary.map(|s| clean_xml_chars(&s));
        let text_summary = match &summary {
            Some(s) if s.contains('<') => {
                let dom = Dom::parse(s);
                clean_ascii_chars(&dom.text_content(dom.root))
            }
            Some(s) => clean_ascii_chars(s),
            None => String::new(),
        };

        Article {
            id: id.into(),
            title,
            url,
            author,
            summary,
            text_summary,
            content,
            date: published.unwrap_or_else(Utc::now),
            toc_thumbnail: None,
            internal_toc_entries: Vec::new(),
            downloaded: false,
        }
    }

    /// Port of the `formatted_date` property (UTC, see this struct's
    /// own `date` field doc for the disclosed localtime narrowing).
    pub fn formatted_date(&self) -> String {
        self.date.format(" [%a, %d %b %H:%M]").to_string()
    }

    /// Port of `is_same_as`.
    pub fn is_same_as(&self, other: &Article) -> bool {
        match &self.url {
            Some(url) => Some(url) == other.url.as_ref(),
            None => self.content == other.content,
        }
    }
}

/// A single item as `feeds_from_index` receives them (real Python
/// duck-types an arbitrary dict here; this is that dict's real shape,
/// made explicit) -- produced by `BasicNewsRecipe.parse_index()`
/// (issue #619+, not yet ported).
#[derive(Debug, Clone, Default)]
pub struct PreparsedArticle {
    pub id: Option<String>,
    pub title: Option<String>,
    pub url: Option<String>,
    pub description: Option<String>,
    pub content: Option<String>,
    pub author: Option<String>,
    pub timestamp: Option<DateTime<Utc>>,
}

/// Port of `Feed`.
#[derive(Debug, Clone)]
pub struct Feed {
    pub title: String,
    pub description: String,
    pub image_url: Option<String>,
    pub image_width: u32,
    pub image_height: u32,
    pub image_alt: String,
    pub articles: Vec<Article>,
    pub oldest_article: f64,
    added_articles: HashSet<String>,
    id_counter: u64,
}

impl Feed {
    fn empty(oldest_article: f64) -> Self {
        Feed {
            title: String::new(),
            description: String::new(),
            image_url: None,
            image_width: 88,
            image_height: 31,
            image_alt: String::new(),
            articles: Vec::new(),
            oldest_article,
            added_articles: HashSet::new(),
            id_counter: 0,
        }
    }

    /// Port of `populate_from_feed`. `get_article_url` replaces real
    /// Python's default `lambda item: item.get('link', None)` --
    /// pass a closure that inspects `entry.links` for callers needing
    /// upstream's `get_article_url` override hook (issue #619's
    /// `BasicNewsRecipe.get_article_url`).
    pub fn populate_from_feed(feed: &feed_rs::model::Feed, title: Option<&str>, oldest_article: f64, max_articles_per_feed: usize, get_article_url: impl Fn(&feed_rs::model::Entry) -> Option<String>) -> Self {
        let mut this = Feed::empty(oldest_article);
        this.title = title.map(str::to_string).unwrap_or_else(|| feed.title.as_ref().map(|t| t.content.clone()).unwrap_or_else(|| "Unknown section".to_string()));
        this.description = feed.description.as_ref().map(|t| t.content.clone()).unwrap_or_default();
        if let Some(image) = &feed.logo {
            this.image_url = Some(image.uri.clone());
            this.image_width = image.width.unwrap_or(88);
            this.image_height = image.height.unwrap_or(31);
            this.image_alt = image.title.clone().unwrap_or_default();
        }

        for entry in &feed.entries {
            if this.articles.len() >= max_articles_per_feed {
                break;
            }
            this.parse_article(entry, &get_article_url);
        }
        this
    }

    /// Port of `populate_from_preparsed_feed`.
    pub fn populate_from_preparsed_feed(title: Option<&str>, articles: &[PreparsedArticle], oldest_article: f64, max_articles_per_feed: usize) -> Self {
        let mut this = Feed::empty(oldest_article);
        this.title = title.map(str::to_string).unwrap_or_else(|| "Unknown feed".to_string());

        for item in articles {
            if this.articles.len() >= max_articles_per_feed {
                break;
            }
            this.id_counter += 1;
            let id = item.id.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| format!("internal id#{}", this.id_counter));
            if this.added_articles.contains(&id) {
                return this;
            }
            this.added_articles.insert(id.clone());

            let published = item.timestamp;
            let article = Article::new(id, item.title.as_deref().or(Some("Untitled article")), item.url.clone(), item.author.clone(), item.description.clone(), published, item.content.clone());
            if within_oldest_article(&article, oldest_article) {
                this.articles.push(article);
            }
        }
        this
    }

    /// Port of `parse_article`.
    fn parse_article(&mut self, item: &feed_rs::model::Entry, get_article_url: &impl Fn(&feed_rs::model::Entry) -> Option<String>) {
        self.id_counter += 1;
        let id = if item.id.is_empty() { format!("internal id#{}", self.id_counter) } else { item.id.clone() };
        if self.added_articles.contains(&id) {
            return;
        }
        let published = item.published.or(item.updated);

        let mut title = item.title.as_ref().map(|t| t.content.clone()).unwrap_or_else(|| "Untitled article".to_string());
        if title.starts_with('<') {
            title = strip_tags(&title);
        }

        let link = get_article_url(item);
        let summary = item.summary.as_ref().map(|t| t.content.clone());
        let author = item.authors.first().map(|p| p.name.clone());
        let content = item.content.as_ref().and_then(|c| c.body.clone()).filter(|c| !c.trim().is_empty());

        if link.is_none() && content.is_none() {
            return;
        }
        self.added_articles.insert(id.clone());
        let article = Article::new(id, Some(&title), link, author, summary, published, content);
        if within_oldest_article(&article, self.oldest_article) {
            self.articles.push(article);
        }
    }

    pub fn reverse(&mut self) {
        self.articles.reverse();
    }

    pub fn len(&self) -> usize {
        self.articles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.articles.is_empty()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Article> {
        self.articles.iter()
    }

    /// Port of `has_embedded_content`.
    pub fn has_embedded_content(&self) -> bool {
        let mut length = 0usize;
        for a in &self.articles {
            if a.content.is_some() || a.summary.is_some() {
                let content_len = a.content.as_deref().map(str::len).unwrap_or(0);
                let summary_len = a.summary.as_deref().map(str::len).unwrap_or(0);
                length += content_len.max(summary_len);
            }
        }
        length > 2000 * self.articles.len()
    }

    /// Port of `has_article`.
    pub fn has_article(&self, article: &Article) -> bool {
        self.articles.iter().any(|a| a.is_same_as(article))
    }

    /// Port of `find`.
    pub fn find(&self, article: &Article) -> Option<usize> {
        self.articles.iter().position(|a| a.is_same_as(article))
    }
}

impl<'a> IntoIterator for &'a Feed {
    type Item = &'a Article;
    type IntoIter = std::slice::Iter<'a, Article>;
    fn into_iter(self) -> Self::IntoIter {
        self.articles.iter()
    }
}

fn within_oldest_article(article: &Article, oldest_article_days: f64) -> bool {
    let delta = Utc::now().signed_duration_since(article.date);
    delta.num_seconds() as f64 <= 24.0 * 3600.0 * oldest_article_days
}

fn strip_tags(s: &str) -> String {
    let dom = Dom::parse(s);
    dom.text_content(dom.root)
}

/// Port of `FeedCollection`.
#[derive(Debug, Clone)]
pub struct FeedCollection {
    pub feeds: Vec<Feed>,
    /// `(article, feed_index)` pairs removed as cross-feed duplicates.
    pub duplicates: Vec<(Article, usize)>,
}

impl FeedCollection {
    /// Port of `FeedCollection.__init__`: drops empty feeds, then
    /// removes any article that's a duplicate (per [`Article::is_same_as`])
    /// of one already seen in an earlier feed.
    pub fn new(feeds: Vec<Feed>) -> Self {
        let mut kept: Vec<Feed> = feeds.into_iter().filter(|f| !f.articles.is_empty()).collect();
        let mut found: Vec<Article> = Vec::new();
        let mut duplicates: Vec<(Article, usize)> = Vec::new();

        for (feed_index, feed) in kept.iter_mut().enumerate() {
            let mut dup_indices = Vec::new();
            for (i, a) in feed.articles.iter().enumerate() {
                if let Some(first) = found.iter().find(|x| a.is_same_as(x)) {
                    duplicates.push((first.clone(), feed_index));
                    dup_indices.push(i);
                } else {
                    found.push(a.clone());
                }
            }
            for i in dup_indices.into_iter().rev() {
                feed.articles.remove(i);
            }
        }

        FeedCollection { feeds: kept, duplicates }
    }

    /// Port of `find_article`.
    pub fn find_article(&self, article: &Article) -> Option<(usize, usize)> {
        for (j, f) in self.feeds.iter().enumerate() {
            for (i, a) in f.articles.iter().enumerate() {
                if std::ptr::eq(a, article) {
                    return Some((j, i));
                }
            }
        }
        None
    }

    /// Port of `restore_duplicates`. Real Python re-finds each
    /// duplicate's original position via object identity
    /// (`find_article`); this port instead re-derives it via
    /// `is_same_as` (Rust's `Vec::remove` invalidates raw pointers, so
    /// identity comparison the way real Python's is-based
    /// `find_article` works isn't available here) -- same real
    /// observable result (the duplicate's `url` is rewritten to point
    /// at wherever its original now lives).
    pub fn restore_duplicates(&mut self) {
        let mut to_restore: Vec<(usize, Article)> = Vec::new();
        for (article, feed_index) in &self.duplicates {
            if let Some((j, i)) = self.feeds.iter().enumerate().find_map(|(j, f)| f.articles.iter().position(|a| a.is_same_as(article)).map(|i| (j, i))) {
                let mut restored = article.clone();
                restored.url = Some(format!("../feed_{j}/article_{i}/index.html"));
                to_restore.push((*feed_index, restored));
            }
        }
        for (feed_index, article) in to_restore {
            self.feeds[feed_index].articles.push(article);
        }
    }
}

/// Port of `feed_from_xml`.
pub fn feed_from_xml(raw_xml: &[u8], title: Option<&str>, oldest_article: f64, max_articles_per_feed: usize, get_article_url: impl Fn(&feed_rs::model::Entry) -> Option<String>) -> Result<Feed, feed_rs::parser::ParseFeedError> {
    // Real Python: handle unclosed escaped entities that trip up the
    // parser (some feeds, e.g. HBR, generate them).
    let fixed = fix_unclosed_entities(raw_xml);
    let feed = feed_rs::parser::parse(std::io::Cursor::new(fixed))?;
    Ok(Feed::populate_from_feed(&feed, title, oldest_article, max_articles_per_feed, get_article_url))
}

fn fix_unclosed_entities(raw_xml: &[u8]) -> Vec<u8> {
    static RE: std::sync::OnceLock<regex::bytes::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::bytes::Regex::new(r"(&amp;#\d+)([^0-9;])").unwrap());
    re.replace_all(raw_xml, &b"$1;$2"[..]).into_owned()
}

/// Port of `feeds_from_index`.
pub fn feeds_from_index(index: &[(String, Vec<PreparsedArticle>)], oldest_article: f64, max_articles_per_feed: usize) -> Vec<Feed> {
    index.iter().map(|(title, articles)| Feed::populate_from_preparsed_feed(Some(title), articles, oldest_article, max_articles_per_feed)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn article(id: &str, url: Option<&str>) -> Article {
        Article::new(id, Some("A Title"), url.map(str::to_string), None, None, None, None)
    }

    #[test]
    fn article_new_falls_back_to_unknown_title_and_cleans_it() {
        let a = Article::new("1", None, None, None, None, None, None);
        assert_eq!(a.title, "Unknown");
        let a2 = Article::new("2", Some(""), None, None, None, None, None);
        assert_eq!(a2.title, "Unknown");
    }

    #[test]
    fn article_new_extracts_plain_text_summary_from_html() {
        let a = Article::new("1", Some("T"), None, None, Some("Hello <b>bold</b> &amp; more".to_string()), None, None);
        assert_eq!(a.text_summary, "Hello bold & more");
    }

    #[test]
    fn article_new_keeps_plain_text_summary_unchanged() {
        let a = Article::new("1", Some("T"), None, None, Some("Just plain text".to_string()), None, None);
        assert_eq!(a.text_summary, "Just plain text");
    }

    #[test]
    fn is_same_as_compares_by_url_when_present_else_content() {
        let a1 = article("1", Some("http://x/"));
        let a2 = article("2", Some("http://x/"));
        let a3 = article("3", Some("http://y/"));
        assert!(a1.is_same_as(&a2));
        assert!(!a1.is_same_as(&a3));

        let mut b1 = article("1", None);
        let mut b2 = article("2", None);
        b1.content = Some("same".to_string());
        b2.content = Some("same".to_string());
        assert!(b1.is_same_as(&b2));
        b2.content = Some("different".to_string());
        assert!(!b1.is_same_as(&b2));
    }

    #[test]
    fn populate_from_preparsed_feed_dedupes_by_id_and_respects_max_articles() {
        let articles = vec![
            PreparsedArticle { id: Some("a".to_string()), title: Some("A".to_string()), timestamp: Some(Utc::now()), ..Default::default() },
            PreparsedArticle { id: Some("a".to_string()), title: Some("A dup".to_string()), timestamp: Some(Utc::now()), ..Default::default() },
            PreparsedArticle { id: Some("b".to_string()), title: Some("B".to_string()), timestamp: Some(Utc::now()), ..Default::default() },
        ];
        // Real Python bails out of the whole loop on the first
        // duplicate id it sees (`return` inside the for-loop, not
        // `continue`) -- match that exactly.
        let feed = Feed::populate_from_preparsed_feed(Some("T"), &articles, 7.0, 100);
        assert_eq!(feed.articles.len(), 1);
        assert_eq!(feed.articles[0].title, "A");
    }

    #[test]
    fn populate_from_preparsed_feed_filters_articles_older_than_oldest_article() {
        let old = Utc::now() - chrono::Duration::days(30);
        let articles = vec![
            PreparsedArticle { id: Some("old".to_string()), title: Some("Old".to_string()), timestamp: Some(old), ..Default::default() },
            PreparsedArticle { id: Some("new".to_string()), title: Some("New".to_string()), timestamp: Some(Utc::now()), ..Default::default() },
        ];
        let feed = Feed::populate_from_preparsed_feed(Some("T"), &articles, 7.0, 100);
        assert_eq!(feed.articles.len(), 1);
        assert_eq!(feed.articles[0].title, "New");
    }

    #[test]
    fn has_embedded_content_matches_the_real_threshold() {
        let mut feed = Feed::empty(7.0);
        let mut a = article("1", None);
        a.content = Some("x".repeat(3000));
        feed.articles.push(a);
        assert!(feed.has_embedded_content(), "3000 > 2000*1");

        let mut feed2 = Feed::empty(7.0);
        let mut a2 = article("1", None);
        a2.content = Some("x".repeat(1000));
        feed2.articles.push(a2);
        assert!(!feed2.has_embedded_content(), "1000 <= 2000*1");
    }

    #[test]
    fn feed_collection_removes_cross_feed_duplicates() {
        let mut f1 = Feed::empty(7.0);
        f1.articles.push(article("1", Some("http://x/")));
        let mut f2 = Feed::empty(7.0);
        f2.articles.push(article("2", Some("http://x/"))); // same URL -> duplicate of f1's article
        f2.articles.push(article("3", Some("http://y/")));

        let fc = FeedCollection::new(vec![f1, f2]);
        assert_eq!(fc.feeds[0].articles.len(), 1);
        assert_eq!(fc.feeds[1].articles.len(), 1); // the duplicate was removed, "http://y/" remains
        assert_eq!(fc.duplicates.len(), 1);
    }

    #[test]
    fn feed_collection_drops_empty_feeds() {
        let f1 = Feed::empty(7.0);
        let mut f2 = Feed::empty(7.0);
        f2.articles.push(article("1", Some("http://x/")));
        let fc = FeedCollection::new(vec![f1, f2]);
        assert_eq!(fc.feeds.len(), 1);
    }

    #[test]
    fn restore_duplicates_appends_a_rewritten_copy_back_to_its_feed() {
        let mut f1 = Feed::empty(7.0);
        f1.articles.push(article("1", Some("http://x/")));
        let mut f2 = Feed::empty(7.0);
        f2.articles.push(article("2", Some("http://x/")));

        let mut fc = FeedCollection::new(vec![f1, f2]);
        assert_eq!(fc.duplicates.len(), 1);
        fc.restore_duplicates();
        assert_eq!(fc.feeds[1].articles.len(), 1);
        assert_eq!(fc.feeds[1].articles[0].url.as_deref(), Some("../feed_0/article_0/index.html"));
    }

    #[test]
    fn feed_from_xml_parses_a_real_rss_feed() {
        let xml = br#"<?xml version="1.0"?>
<rss version="2.0">
<channel>
<title>Test Feed</title>
<description>A test feed</description>
<item>
<title>First Article</title>
<link>http://example.com/1</link>
<description>Summary one</description>
<pubDate>Mon, 01 Jan 2024 12:00:00 GMT</pubDate>
</item>
<item>
<title>Second Article</title>
<link>http://example.com/2</link>
<description>Summary two</description>
<pubDate>Tue, 02 Jan 2024 12:00:00 GMT</pubDate>
</item>
</channel>
</rss>"#;
        let feed = feed_from_xml(xml, None, 36500.0, 100, |e| e.links.first().map(|l| l.href.clone())).expect("real RSS feed must parse");
        assert_eq!(feed.title, "Test Feed");
        assert_eq!(feed.articles.len(), 2);
        assert_eq!(feed.articles[0].title, "First Article");
        assert_eq!(feed.articles[0].url.as_deref(), Some("http://example.com/1"));
        assert_eq!(feed.articles[1].title, "Second Article");
    }

    #[test]
    fn feed_from_xml_fixes_unclosed_numeric_entities() {
        // A raw `&#38` (no trailing `;`) followed by a non-digit would
        // trip up a strict XML parser; real Python patches this before
        // handing the bytes to feedparser.
        let xml = br#"<?xml version="1.0"?>
<rss version="2.0"><channel><title>T&amp;#38 &amp;More</title>
<item><title>A</title><link>http://x/</link><description>d</description></item>
</channel></rss>"#;
        let feed = feed_from_xml(xml, None, 36500.0, 100, |e| e.links.first().map(|l| l.href.clone())).expect("must still parse after the entity fix-up");
        assert_eq!(feed.articles.len(), 1);
    }

    #[test]
    fn feeds_from_index_builds_one_feed_per_section() {
        let index = vec![
            ("Section A".to_string(), vec![PreparsedArticle { id: Some("1".to_string()), title: Some("A1".to_string()), timestamp: Some(Utc::now()), ..Default::default() }]),
            ("Section B".to_string(), vec![PreparsedArticle { id: Some("2".to_string()), title: Some("B1".to_string()), timestamp: Some(Utc::now()), ..Default::default() }]),
        ];
        let feeds = feeds_from_index(&index, 7.0, 100);
        assert_eq!(feeds.len(), 2);
        assert_eq!(feeds[0].title, "Section A");
        assert_eq!(feeds[1].title, "Section B");
    }
}
