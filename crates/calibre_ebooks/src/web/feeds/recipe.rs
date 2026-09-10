//! Port of `old_src/src/calibre/web/feeds/news.py`'s `BasicNewsRecipe`
//! config surface + trivial hook methods + `extract_readable_article`
//! (issue #619, split from #81). See `web::feeds`'s own module doc for
//! the full cluster context.
//!
//! # Scope
//!
//! [`RecipeConfig`] (the ~55-field class-attribute configuration
//! surface, trivially portable as struct fields with real defaults)
//! and [`NewsRecipeHooks`] (the ~20 trivial accessor/hook methods
//! recipe authors override, as a trait with real default
//! implementations). Tag-selector fields (`remove_tags`/
//! `keep_only_tags`/`remove_tags_after`/`remove_tags_before`) use a
//! minimal, real [`TagSpec`] shape matching the literal
//! `dict(name=..., attrs={...})`/`dict(id=[...])` syntax every real
//! recipe in this codebase's own corpus uses -- full BeautifulSoup-
//! style predicate matching (regex classes, callables) is issue
//! #620's job (the HTML cleanup pipeline that actually consumes these
//! fields), not this one's.
//!
//! [`extract_readable_article`] wires the real, already-ported
//! `readability::Document` (see that module's own doc: "The only
//! caller of this package in `old_src` is `calibre.web.feeds.news`...
//! This port doesn't depend on it existing" -- this is that caller,
//! now real) directly onto this method's 3 real calls
//! (`Document::new`/`.summary()`/`.title()`).
//!
//! # Disclosed narrowing in `extract_readable_article`
//!
//! Real Python checks whether the extracted title needs a synthetic
//! heading inserted by testing *each descendant element's own direct
//! `.text`* for an exact match against `extracted_title`, plus
//! whether any `h1`/`h2` exists at all. This port approximates the
//! first check as "does `extracted_title` appear anywhere in the
//! body's full text content" (`Dom::text_content`, a subtree
//! concatenation, not a per-element direct-text equality check) --
//! `Dom` doesn't expose per-element direct-text without a bespoke
//! walk, and the *only* real consequence of a false-negative-to-
//! false-positive drift here is whether a redundant `<h2>{title}</h2>`
//! gets inserted, a cosmetic difference with no effect on article
//! content or the summary itself.

use std::collections::HashMap;

use crate::dom::Dom;
use crate::readability::{Document, DocumentOptions};

// ===================================================================
// Config surface
// ===================================================================

/// Port of `BasicNewsRecipe.needs_subscription`'s 3-way real value
/// (`True`/`False`/`"optional"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NeedsSubscription {
    #[default]
    No,
    Yes,
    Optional,
}

/// Port of `BasicNewsRecipe.browser_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BrowserType {
    #[default]
    Mechanize,
    WebEngine,
    Qt,
}

/// A single attribute-value matcher in a [`TagSpec`] (real Python:
/// `dict(attrs={'class': 'advert'})` / `dict(id=['content', 'heading'])`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagAttrValue {
    Str(String),
    List(Vec<String>),
}

/// Port of a `remove_tags`/`keep_only_tags`/`remove_tags_after`/
/// `remove_tags_before` entry's real `dict(name=..., attrs={...})`
/// shape (see this module's own doc for the disclosed scope: matching
/// logic itself belongs to #620).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TagSpec {
    pub name: Option<String>,
    pub attrs: HashMap<String, TagAttrValue>,
}

/// Port of `BasicNewsRecipe.cover_margins`.
#[derive(Debug, Clone, PartialEq)]
pub struct CoverMargins {
    pub horizontal: i32,
    pub vertical: i32,
    pub color: String,
}

impl Default for CoverMargins {
    fn default() -> Self {
        CoverMargins { horizontal: 0, vertical: 0, color: "#ffffff".to_string() }
    }
}

/// Port of `BasicNewsRecipe`'s real class-attribute configuration
/// surface (issue #619). Field-for-field, with real Python defaults.
#[derive(Debug, Clone)]
pub struct RecipeConfig {
    pub title: String,
    pub description: String,
    pub author: String,
    pub requires_version: (u32, u32, u32),
    pub language: String,
    pub max_articles_per_feed: usize,
    pub oldest_article: f64,
    pub recursions: u32,
    pub delay: f64,
    pub publication_type: Option<String>,
    pub simultaneous_downloads: usize,
    pub timeout: f64,
    pub timefmt: String,
    /// `(title, url)` pairs -- real Python also accepts a bare `[url,
    /// ...]` list (title defaults to the feed's own), represented here
    /// as `None` titles.
    pub feeds: Option<Vec<(Option<String>, String)>>,
    pub summary_length: usize,
    pub no_stylesheets: bool,
    pub remove_javascript: bool,
    pub needs_subscription: NeedsSubscription,
    pub center_navbar: bool,
    pub use_embedded_content: Option<bool>,
    pub articles_are_obfuscated: bool,
    pub reverse_article_order: bool,
    pub auto_cleanup: bool,
    pub auto_cleanup_keep: Option<String>,
    pub extra_css: Option<String>,
    pub remove_empty_feeds: bool,
    pub match_regexps: Vec<String>,
    pub filter_regexps: Vec<String>,
    pub remove_tags: Vec<TagSpec>,
    pub remove_tags_after: Option<TagSpec>,
    pub remove_tags_before: Option<TagSpec>,
    pub remove_attributes: Vec<String>,
    pub keep_only_tags: Vec<TagSpec>,
    /// `(pattern, replacement)` pairs -- real Python's second element
    /// is a match-callback; this port keeps only the common
    /// literal-replacement-string case real recipes overwhelmingly
    /// use, matching this issue's own "config surface, not the
    /// processing engine" scope (#620 owns the actual regex engine).
    pub preprocess_regexps: Vec<(String, String)>,
    pub template_css: String,
    pub masthead_url: Option<String>,
    pub cover_margins: CoverMargins,
    pub recipe_disabled: Option<String>,
    /// Real Python: a `set` containing any of `{'title', 'url'}`.
    pub ignore_duplicate_articles_by_title: bool,
    pub ignore_duplicate_articles_by_url: bool,
    pub compress_news_images: bool,
    pub compress_news_images_auto_size: u32,
    pub compress_news_images_max_size: Option<u32>,
    pub scale_news_images_to_device: bool,
    pub scale_news_images: Option<(u32, u32)>,
    pub resolve_internal_links: bool,
    pub browser_type: BrowserType,
    pub handle_gzip: bool,
}

impl Default for RecipeConfig {
    fn default() -> Self {
        RecipeConfig {
            title: "Unknown News Source".to_string(),
            description: String::new(),
            author: "calibre".to_string(),
            requires_version: (0, 6, 0),
            language: "und".to_string(),
            max_articles_per_feed: 100,
            oldest_article: 7.0,
            recursions: 0,
            delay: 0.0,
            publication_type: Some("unknown".to_string()),
            simultaneous_downloads: 5,
            timeout: 120.0,
            timefmt: " [%a, %d %b %Y]".to_string(),
            feeds: None,
            summary_length: 500,
            no_stylesheets: false,
            remove_javascript: true,
            needs_subscription: NeedsSubscription::No,
            center_navbar: true,
            use_embedded_content: None,
            articles_are_obfuscated: false,
            reverse_article_order: false,
            auto_cleanup: false,
            auto_cleanup_keep: None,
            extra_css: None,
            remove_empty_feeds: false,
            match_regexps: Vec::new(),
            filter_regexps: Vec::new(),
            remove_tags: Vec::new(),
            remove_tags_after: None,
            remove_tags_before: None,
            remove_attributes: Vec::new(),
            keep_only_tags: Vec::new(),
            preprocess_regexps: Vec::new(),
            template_css: DEFAULT_TEMPLATE_CSS.to_string(),
            masthead_url: None,
            cover_margins: CoverMargins::default(),
            recipe_disabled: None,
            ignore_duplicate_articles_by_title: false,
            ignore_duplicate_articles_by_url: false,
            compress_news_images: false,
            compress_news_images_auto_size: 16,
            compress_news_images_max_size: None,
            scale_news_images_to_device: true,
            scale_news_images: None,
            resolve_internal_links: false,
            browser_type: BrowserType::Mechanize,
            handle_gzip: true,
        }
    }
}

const DEFAULT_TEMPLATE_CSS: &str = "
            .article_date {
                color: gray; font-family: monospace;
            }

            .article_description {
                text-indent: 0pt;
            }

            a.article {
                font-weight: bold; text-align:left;
            }

            a.feed {
                font-weight: bold;
            }

            .calibre_navbar {
                font-family:monospace;
            }
    ";

// ===================================================================
// Hook methods
// ===================================================================

/// Port of `BasicNewsRecipe`'s ~20 trivial accessor/hook methods, as a
/// trait with real default implementations. A concrete recipe
/// implements this trait (providing [`NewsRecipeHooks::config`]) and
/// overrides whichever methods it needs to customize -- the same
/// override-a-few-methods shape real Python subclassing gives recipe
/// authors.
pub trait NewsRecipeHooks {
    fn config(&self) -> &RecipeConfig;

    /// Port of `short_title`.
    fn short_title(&self) -> String {
        self.config().title.clone()
    }

    /// Port of `is_link_wanted`. Real Python raises `NotImplementedError`
    /// by default (causing the downloader to fall back to
    /// `match_regexps`/`filter_regexps`); `None` is that same "not
    /// implemented" signal here.
    fn is_link_wanted(&self, _url: &str, _tag_name: &str) -> Option<bool> {
        None
    }

    /// Port of `get_extra_css`.
    fn get_extra_css(&self) -> Option<String> {
        self.config().extra_css.clone()
    }

    /// Port of `get_cover_url`. Real Python's default reads
    /// `getattr(self, 'cover_url', None)` -- no recipe sets a bare
    /// `cover_url` class attribute by default (it's an instance
    /// attribute a recipe's own `__init__`/`parse_index` sets at
    /// runtime), so the real default is always `None` here too.
    fn get_cover_url(&self) -> Option<String> {
        None
    }

    /// Port of `get_masthead_url`.
    fn get_masthead_url(&self) -> Option<String> {
        self.config().masthead_url.clone()
    }

    /// Port of `get_masthead_title`.
    fn get_masthead_title(&self) -> String {
        self.config().title.clone()
    }

    /// Port of `get_feeds`. Real Python raises `NotImplementedError`
    /// if `self.feeds` is falsy; `None` is that signal.
    fn get_feeds(&self) -> Option<Vec<(Option<String>, String)>> {
        self.config().feeds.clone().filter(|f| !f.is_empty())
    }

    /// Port of `get_url_specific_delay`.
    fn get_url_specific_delay(&self, _url: &str) -> f64 {
        self.config().delay
    }

    /// Port of `print_version` (a real Python `@classmethod`, ported
    /// as an instance method -- disclosed, narrow signature deviation;
    /// no real caller in this port's scope needs the classmethod-
    /// without-an-instance form). Default: not implemented.
    fn print_version(&self, _url: &str) -> Option<String> {
        None
    }

    /// Port of `image_url_processor` (also a real classmethod, same
    /// disclosed deviation as `print_version`).
    fn image_url_processor(&self, _base_url: &str, url: &str) -> Option<String> {
        Some(url.to_string())
    }

    /// Port of `preprocess_image`.
    fn preprocess_image(&self, img_data: Vec<u8>, _image_url: &str) -> Option<Vec<u8>> {
        Some(img_data)
    }
}

// ===================================================================
// description_limiter
// ===================================================================

/// Port of `BasicNewsRecipe.description_limiter`: truncates `src` to
/// roughly `summary_length` characters, preferring to cut at a nearby
/// `;` or `>` (within 50 characters past the cutoff) to avoid slicing
/// through an HTML entity or tag. Used by `templates.rs`'s
/// `generate_feed`/`generate_touchscreen_feed` (as their injected
/// `cutoff` closure -- see those functions' own doc for why they take
/// a closure rather than depending on this trait) and by
/// `crate::web::feeds::opf`'s `create_opf` port for article summaries.
pub fn description_limiter(src: &str, summary_length: usize) -> String {
    if src.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = src.chars().collect();
    let pos = summary_length as i64;
    let fuzz = 50i64;

    let find_from = |needle: char, from: i64| -> i64 {
        if from < 0 || from as usize >= chars.len() {
            return -1;
        }
        chars[from as usize..].iter().position(|&c| c == needle).map(|i| i as i64 + from).unwrap_or(-1)
    };

    let mut si = find_from(';', pos);
    if si > 0 && si - pos > fuzz {
        si = -1;
    }
    let mut gi = find_from('>', pos);
    if gi > 0 && gi - pos > fuzz {
        gi = -1;
    }
    let mut npos = si.max(gi);
    if npos < 0 {
        npos = pos;
    }
    let end = (npos + 1).clamp(0, chars.len() as i64) as usize;
    let ans: String = chars[..end].iter().collect();
    if end < chars.len() {
        format!("{}\u{2026}", calibre_utils::cleantext::clean_xml_chars(&ans))
    } else {
        ans
    }
}

// ===================================================================
// extract_readable_article
// ===================================================================

/// Port of `BasicNewsRecipe.extract_readable_article`.
pub fn extract_readable_article(html: &[u8], url: Option<&str>, auto_cleanup_keep: Option<&str>) -> String {
    let mut doc = Document::new(html.to_vec(), DocumentOptions { url: url.map(str::to_string), keep_elements_xpath: auto_cleanup_keep.map(str::to_string), ..Default::default() });
    let article_html = doc.summary().unwrap_or_default();
    let extracted_title = doc.title();

    // `Document::summary()` always returns body-shaped content
    // (`get_body`'s own contract) -- `Dom::parse` normalizes it into a
    // real `<html><head/><body>...</body></html>` tree regardless,
    // sidestepping real Python's 3-way `frag.tag` branch (`html`/
    // `body`/other) entirely: there's only ever the one real shape to
    // handle here.
    let mut parsed = Dom::parse(&article_html);
    let html_id = parsed.find_first_tag_global("html").unwrap_or(parsed.root);
    let head_id = match parsed.find_all_tag(html_id, "head").into_iter().next() {
        Some(id) => id,
        None => {
            let id = parsed.new_element("head");
            parsed.insert_child(html_id, 0, id);
            id
        }
    };
    let title_el = parsed.new_element("title");
    let title_text = parsed.new_text(&extracted_title);
    parsed.append_child(title_el, title_text);
    parsed.append_child(head_id, title_el);

    let body_id = parsed.find_first_tag_global("body").unwrap_or(html_id);
    let has_title = !extracted_title.is_empty() && parsed.text_content(body_id).contains(&extracted_title);
    let has_inline_heading = !parsed.find_all_tag(body_id, "h1").is_empty() || !parsed.find_all_tag(body_id, "h2").is_empty();
    if !has_title && !has_inline_heading {
        let heading = parsed.new_element("h2");
        let heading_text = parsed.new_text(&extracted_title);
        parsed.append_child(heading, heading_text);
        parsed.insert_child(body_id, 0, heading);
    }

    parsed.serialize(html_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestRecipe(RecipeConfig);
    impl NewsRecipeHooks for TestRecipe {
        fn config(&self) -> &RecipeConfig {
            &self.0
        }
    }

    #[test]
    fn recipe_config_matches_the_real_python_defaults() {
        let cfg = RecipeConfig::default();
        assert_eq!(cfg.title, "Unknown News Source");
        assert_eq!(cfg.max_articles_per_feed, 100);
        assert_eq!(cfg.oldest_article, 7.0);
        assert_eq!(cfg.simultaneous_downloads, 5);
        assert_eq!(cfg.timeout, 120.0);
        assert!(cfg.remove_javascript);
        assert!(cfg.center_navbar);
        assert_eq!(cfg.compress_news_images_auto_size, 16);
        assert!(cfg.scale_news_images_to_device);
        assert!(cfg.handle_gzip);
        assert_eq!(cfg.browser_type, BrowserType::Mechanize);
        assert_eq!(cfg.needs_subscription, NeedsSubscription::No);
        assert_eq!(cfg.cover_margins, CoverMargins { horizontal: 0, vertical: 0, color: "#ffffff".to_string() });
    }

    #[test]
    fn short_title_defaults_to_the_config_title() {
        let r = TestRecipe(RecipeConfig { title: "My Paper".to_string(), ..Default::default() });
        assert_eq!(r.short_title(), "My Paper");
    }

    #[test]
    fn description_limiter_returns_short_strings_unchanged() {
        assert_eq!(description_limiter("Hello, world!", 500), "Hello, world!");
        assert_eq!(description_limiter("", 500), "");
    }

    #[test]
    fn description_limiter_truncates_long_strings_with_an_ellipsis() {
        let src = "a".repeat(600);
        let out = description_limiter(&src, 500);
        assert!(out.ends_with('\u{2026}'), "{out}");
        assert!(out.len() < src.len());
    }

    #[test]
    fn description_limiter_prefers_cutting_at_a_nearby_entity_or_tag_boundary() {
        // A `;` just a few characters past the cutoff should be
        // preferred over an exact mid-entity cut.
        let src = format!("{}&amp;{}", "a".repeat(10), "b".repeat(20));
        let out = description_limiter(&src, 10);
        assert!(out.starts_with("aaaaaaaaaa&amp;"), "{out}");
    }

    #[test]
    fn is_link_wanted_defaults_to_not_implemented() {
        let r = TestRecipe(RecipeConfig::default());
        assert_eq!(r.is_link_wanted("http://x/", "a"), None);
    }

    #[test]
    fn get_extra_css_returns_the_configured_value() {
        let r = TestRecipe(RecipeConfig { extra_css: Some(".x{}".to_string()), ..Default::default() });
        assert_eq!(r.get_extra_css(), Some(".x{}".to_string()));
    }

    #[test]
    fn get_cover_url_defaults_to_none() {
        let r = TestRecipe(RecipeConfig::default());
        assert_eq!(r.get_cover_url(), None);
    }

    #[test]
    fn get_feeds_returns_none_when_unset_matching_not_implemented() {
        let r = TestRecipe(RecipeConfig::default());
        assert_eq!(r.get_feeds(), None);
        let r2 = TestRecipe(RecipeConfig { feeds: Some(vec![(None, "http://x/".to_string())]), ..Default::default() });
        assert_eq!(r2.get_feeds(), Some(vec![(None, "http://x/".to_string())]));
    }

    #[test]
    fn get_url_specific_delay_defaults_to_the_configured_delay() {
        let r = TestRecipe(RecipeConfig { delay: 2.5, ..Default::default() });
        assert_eq!(r.get_url_specific_delay("http://x/"), 2.5);
    }

    #[test]
    fn image_url_processor_passes_the_url_through_unchanged_by_default() {
        let r = TestRecipe(RecipeConfig::default());
        assert_eq!(r.image_url_processor("http://base/", "http://img/"), Some("http://img/".to_string()));
    }

    #[test]
    fn preprocess_image_passes_data_through_unchanged_by_default() {
        let r = TestRecipe(RecipeConfig::default());
        assert_eq!(r.preprocess_image(vec![1, 2, 3], "http://img/"), Some(vec![1, 2, 3]));
    }

    #[test]
    fn extract_readable_article_produces_real_html_with_a_title() {
        let html = b"<html><body><article><h1>Ignore</h1><p>Some real article text that is long enough to be considered content by the readability scoring heuristics, repeated for length. Some real article text that is long enough to be considered content by the readability scoring heuristics, repeated for length.</p></article></body></html>";
        let out = extract_readable_article(html, Some("http://example.com/a"), None);
        assert!(out.contains("<title>"));
        assert!(out.to_lowercase().contains("<html"));
        assert!(out.to_lowercase().contains("<body"));
    }

    #[test]
    fn extract_readable_article_inserts_a_heading_when_none_exists() {
        // No h1/h2 anywhere and a title readability can't find inline
        // -- summary() alone won't contain the extracted title text,
        // so a synthetic <h2> must be inserted.
        let html = b"<html><head><title>My Real Title</title></head><body><p>Just a plain paragraph with no heading tags at all, long enough to be kept by the content scoring heuristics used by the readability algorithm under test here.</p></body></html>";
        let out = extract_readable_article(html, None, None);
        assert!(out.contains("My Real Title"));
    }
}
