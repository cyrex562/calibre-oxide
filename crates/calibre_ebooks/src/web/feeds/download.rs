//! Port of `BasicNewsRecipe.download`/`build_index`/`parse_feeds` and
//! the per-article fetch dispatch (`_fetch_article`/`fetch_article`/
//! `fetch_embedded_article`, `old_src/src/calibre/web/feeds/news.py`,
//! issue #623): the real, end-to-end "download a periodical" entry
//! point, wiring #617 (`Feed`/`Article`/`feed_from_xml`), #618
//! (templates), #619 (`RecipeConfig`/hooks), #620
//! (`postprocess_html`), #621 (cover/masthead), #622 (`create_opf`),
//! and #455's whole `RecursiveFetcher` stack (#630-#633) together.
//! Closes out the entire web/feeds cluster (issue #81).
//!
//! # Scope: disclosed narrowings
//!
//! - **`parse_index()`-based custom index scraping has no hook.**
//!   `NewsRecipeHooks` doesn't model it (a whole separate structured-
//!   HTML-scraping mechanism per recipe, returning a hand-parsed
//!   `(title, [(section, [articles])])` shape) -- [`build_index`]
//!   always falls through to [`parse_feeds`], matching every real
//!   feed-based recipe (the overwhelming majority) that never
//!   overrides `parse_index` in the first place.
//! - **`get_obfuscated_article` has no hook either.** `articles_are_obfuscated`
//!   is therefore unreachable in any real, useful sense in this
//!   port -- if a recipe sets it without this port ever gaining the
//!   matching hook, its articles simply fetch via the plain URL path
//!   instead, a real but harmless narrowing (no recipe corpus file in
//!   this repo is executed by this port at all yet, so there's no
//!   live consumer this could silently break).
//! - **HTTP Basic-auth embedded in a feed URL** (`http://user:pass@host/feed`)
//!   has no equivalent -- [`crate::scraper::Browser`] doesn't model
//!   per-request credentials separately from the URL. Such a feed
//!   fetches without auth and likely 401s, handled identically to any
//!   other feed-fetch failure (an empty/error [`Feed`] placeholder,
//!   not a crash).
//! - **One shared `Browser`, not one per article-fetch thread.** Real
//!   Python clones a separate browser (independent cookie jar) per
//!   article fetch; `Browser` has no clone/fork-with-independent-
//!   cookie-jar operation yet, so this port shares one instance across
//!   every worker thread. Concurrently-running article fetches share
//!   cookies -- only matters for stateful/login-requiring recipes, not
//!   the common read-only news-fetching case.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use regex::Regex;

use crate::scraper::{Browser, OpenOptions};
use crate::web::feeds::postprocess::NewsRecipePostprocessHooks;
use crate::web::feeds::recipe::{NewsRecipeHooks, RecipeConfig};
use crate::web::feeds::templates;
use crate::web::feeds::{feed_from_xml, Article, Feed};
use crate::web::fetch::media::ImageCompressionOptions;
use crate::web::fetch::recursive::{self, FetcherConfig, FetcherContext, FetchState, OwnedNavbarContext};

/// Port of `BasicNewsRecipe.parse_feeds`. `get_article_url` defaults
/// to real Python's own `get_article_url` default (`article.get('link',
/// None)`) since #617 already deliberately took this as an injected
/// closure rather than a `NewsRecipeHooks` method.
pub fn parse_feeds<H: NewsRecipeHooks>(hooks: &H, browser: &Browser) -> Vec<Feed> {
    let cfg = hooks.config();
    let Some(feed_specs) = hooks.get_feeds() else { return Vec::new() };
    let mut feeds = Vec::new();

    for (title, mut url) in feed_specs {
        if let Some(rest) = url.strip_prefix("feed://") {
            url = format!("http{rest}");
        }

        let opts = OpenOptions { timeout: Some(Duration::from_secs_f64(cfg.timeout.max(0.0))), ..Default::default() };
        let feed = match browser.open_novisit(&url, &opts) {
            Ok(resp) => {
                let get_article_url = |entry: &feed_rs::model::Entry| entry.links.first().map(|l| l.href.clone());
                match feed_from_xml(resp.read(), title.as_deref(), cfg.oldest_article, cfg.max_articles_per_feed, get_article_url) {
                    Ok(f) => f,
                    Err(err) => error_feed(title.as_deref(), &url, &err.to_string()),
                }
            }
            Err(err) => error_feed(title.as_deref(), &url, &err.to_string()),
        };
        feeds.push(feed);

        let delay = Duration::from_secs_f64(hooks.get_url_specific_delay(&url).max(0.0));
        if !delay.is_zero() {
            thread::sleep(delay);
        }
    }

    if cfg.remove_empty_feeds {
        feeds.retain(|f| !f.articles.is_empty());
    }
    feeds
}

fn error_feed(title: Option<&str>, url: &str, err: &str) -> Feed {
    let msg = format!("Failed feed: {}", title.unwrap_or(url));
    let mut feed = Feed::populate_from_preparsed_feed(Some(&msg), &[], 36500.0, 0);
    feed.description = err.to_string();
    feed
}

/// Port of `BasicNewsRecipe.remove_duplicate_articles`. Real Python's
/// `ignore_duplicate_articles` is a general `{'title','url'}` set;
/// #619 already narrowed this to the two real booleans
/// `RecipeConfig::ignore_duplicate_articles_by_title`/`_by_url`.
pub fn remove_duplicate_articles(cfg: &RecipeConfig, mut feeds: Vec<Feed>) -> Vec<Feed> {
    if cfg.ignore_duplicate_articles_by_title {
        dedupe_by(&mut feeds, |a| Some(a.title.clone()).filter(|t| !t.is_empty()));
    }
    if cfg.ignore_duplicate_articles_by_url {
        dedupe_by(&mut feeds, |a| a.url.clone().filter(|u| !u.is_empty()));
    }
    if cfg.remove_empty_feeds {
        feeds.retain(|f| !f.articles.is_empty());
    }
    feeds
}

fn dedupe_by(feeds: &mut [Feed], key_of: impl Fn(&Article) -> Option<String>) {
    let mut seen = std::collections::HashSet::new();
    for feed in feeds.iter_mut() {
        feed.articles.retain(|a| match key_of(a) {
            Some(key) => seen.insert(key),
            None => true,
        });
    }
}

/// Port of `feed2index`'s own feed-image-downloading block. Mutates
/// each `Feed::image_url` in place to point at the local, downloaded
/// copy (or leaves it untouched on any failure, matching real
/// Python's own bare `except Exception: pass`).
pub fn download_feed_images(feeds: &mut [Feed], browser: &Browser, timeout: Duration, output_dir: &Path) {
    let imgdir = output_dir.join("images");
    let mut counter = 0u32;
    for feed in feeds.iter_mut() {
        let Some(url) = feed.image_url.clone() else { continue };
        let bn = url.rsplit('/').next().unwrap_or("");
        if bn.is_empty() {
            continue;
        }
        let ext = Path::new(bn).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let _ = std::fs::create_dir_all(&imgdir);
        let img_path = imgdir.join(format!("feed_image_{counter}{ext}"));
        let opts = OpenOptions { timeout: Some(timeout), ..Default::default() };
        if let Ok(resp) = browser.open(&url, &opts) {
            if std::fs::write(&img_path, resp.read()).is_ok() {
                counter += 1;
                feed.image_url = Some(img_path.to_string_lossy().into_owned());
            }
        }
    }
}

/// Port of the `lang_for_html` property.
fn lang_for_html(language: &str) -> Option<String> {
    let lang = language.replace('_', "-");
    let lang = lang.split('-').next().unwrap_or("").to_lowercase();
    if lang.is_empty() || lang == "und" {
        None
    } else {
        Some(lang)
    }
}

/// Port of `BasicNewsRecipe.feeds2index`.
pub fn feeds2index<H: NewsRecipeHooks>(hooks: &H, feeds: &[Feed], touchscreen: bool, masthead_filename: &str, date_str: &str) -> String {
    let cfg = hooks.config();
    let extra_css = format!("{}\n\n{}", cfg.template_css, hooks.get_extra_css().unwrap_or_default());
    let html_lang = lang_for_html(&cfg.language);
    if touchscreen {
        templates::generate_touchscreen_index(&hooks.short_title(), masthead_filename, date_str, feeds, Some(&extra_css), None, html_lang.as_deref())
    } else {
        templates::generate_index(&hooks.short_title(), masthead_filename, date_str, feeds, Some(&extra_css), None, html_lang.as_deref())
    }
}

/// Port of `BasicNewsRecipe.feed2index`.
pub fn feed2index<H: NewsRecipeHooks>(hooks: &H, f: usize, feeds: &[Feed], touchscreen: bool) -> String {
    let cfg = hooks.config();
    let extra_css = format!("{}\n\n{}", cfg.template_css, hooks.get_extra_css().unwrap_or_default());
    let html_lang = lang_for_html(&cfg.language);
    let cutoff = |s: &str| crate::web::feeds::recipe::description_limiter(s, cfg.summary_length);
    if touchscreen {
        templates::generate_touchscreen_feed(f, feeds, cutoff, Some(&extra_css), None, html_lang.as_deref())
    } else {
        templates::generate_feed(f, feeds, cutoff, Some(&extra_css), None, html_lang.as_deref())
    }
}

fn compile_regexps_ignorecase(patterns: &[String]) -> Vec<Regex> {
    patterns.iter().filter_map(|p| Regex::new(&format!("(?i){p}")).ok()).collect()
}

fn compile_preprocess_regexps(patterns: &[(String, String)]) -> Vec<(Regex, String)> {
    patterns.iter().filter_map(|(p, r)| Regex::new(p).ok().map(|re| (re, r.clone()))).collect()
}

fn build_fetcher_config(cfg: &RecipeConfig, touchscreen: bool) -> FetcherConfig {
    FetcherConfig {
        timeout: Duration::from_secs_f64(cfg.timeout.max(0.0)),
        max_recursions: cfg.recursions,
        max_files: usize::MAX,
        match_regexps: compile_regexps_ignorecase(&cfg.match_regexps),
        filter_regexps: compile_regexps_ignorecase(&cfg.filter_regexps),
        preprocess_regexps: compile_preprocess_regexps(&cfg.preprocess_regexps),
        download_stylesheets: !cfg.no_stylesheets,
        encoding: None,
        compression: ImageCompressionOptions {
            compress_news_images: cfg.compress_news_images,
            compress_news_images_max_size_kb: cfg.compress_news_images_max_size,
            compress_news_images_auto_size: cfg.compress_news_images_auto_size,
            scale_news_images: if cfg.scale_news_images_to_device { cfg.scale_news_images } else { None },
        },
        touchscreen,
    }
}

fn write_embedded_source(art_dir: &Path, html: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(art_dir)?;
    let path = art_dir.join("_embedded_source.html");
    std::fs::write(&path, html)?;
    Ok(path)
}

/// The URL (or synthetic `file://` source) and optional preloaded
/// bytes to fetch for one article -- port of the three-way
/// `fetch_embedded_article`/`fetch_obfuscated_article`/`fetch_article`
/// dispatch in `build_index`'s own job-construction loop.
fn resolve_article_source<H: NewsRecipeHooks>(hooks: &H, feed: &Feed, article: &Article, art_dir: &Path) -> Option<(String, Option<Vec<u8>>)> {
    let cfg = hooks.config();
    let use_embedded = cfg.use_embedded_content.unwrap_or_else(|| feed.has_embedded_content());
    if use_embedded {
        let extra_css = hooks.get_extra_css();
        let html = templates::generate_embedded_content(article, None, extra_css.as_deref());
        let path = write_embedded_source(art_dir, &html).ok()?;
        return Some((format!("file://{}", path.display()), None));
    }
    let raw_url = article.url.clone()?;
    let url = hooks.print_version(&raw_url).unwrap_or(raw_url);
    Some((url, None))
}

/// Result of fetching one article -- port of `_fetch_article`'s own
/// `(res, path, failures)` return tuple.
pub struct ArticleFetchResult {
    pub index_path: PathBuf,
    pub downloaded_paths: Vec<PathBuf>,
    pub failed_links: Vec<(String, String)>,
}

/// Port of `_fetch_article`: wires one article's fetch onto #633's
/// `RecursiveFetcher` (`FetcherContext`/`start_fetch`).
#[allow(clippy::too_many_arguments)]
pub fn fetch_article<H: NewsRecipePostprocessHooks>(hooks: &H, browser: &Browser, image_cache: &Mutex<HashMap<String, String>>, stylesheet_cache: &Mutex<HashMap<String, String>>, url: &str, preloaded: Option<Vec<u8>>, art_dir: &Path, feed_index: usize, article_index: usize, feed_len: usize, has_single_feed: bool, touchscreen: bool) -> Result<ArticleFetchResult, String> {
    let config = build_fetcher_config(hooks.config(), touchscreen);
    let job_info = Some(OwnedNavbarContext { url: url.to_string(), feed_index, article_index, feed_len, has_single_feed });
    let ctx = FetcherContext::new(hooks, browser, image_cache, stylesheet_cache, config, job_info);
    if let Some(data) = preloaded {
        ctx.preloaded.lock().unwrap().insert(url.to_string(), data);
    }
    let mut state = FetchState::new(art_dir);

    let res = recursive::start_fetch(&ctx, &mut state, url);
    match res {
        Some(path) if Path::new(&path).exists() => Ok(ArticleFetchResult { index_path: PathBuf::from(path), downloaded_paths: state.downloaded_paths, failed_links: state.failed_links }),
        _ => Err("Could not fetch article.".to_string()),
    }
}

/// One completed (or failed) article-download job -- what the worker
/// pool in [`build_index`] collects, applied to `feeds` only after
/// every worker has finished (so `feeds` is never borrowed both by
/// the worker pool and mutated at the same time).
struct JobOutcome {
    feed_index: usize,
    article_index: usize,
    result: Result<ArticleFetchResult, String>,
}

/// Port of `BasicNewsRecipe.build_index`. The real, end-to-end
/// orchestrator: parses feeds, downloads the cover/masthead (#621),
/// writes `index.html`/`feed_N/index.html` (#618), downloads every
/// article concurrently (bounded worker pool, matching real Python's
/// own `ThreadPool(self.simultaneous_downloads)`) via #633's
/// `RecursiveFetcher`, and assembles the final OPF/NCX (#622).
pub fn build_index<H: NewsRecipePostprocessHooks + Sync>(hooks: &H, browser: &Browser, output_dir: &Path, touchscreen: bool, simultaneous_downloads: usize, db: &Arc<fontdb::Database>) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(output_dir)?;
    let cfg = hooks.config();
    let timeout = Duration::from_secs_f64(cfg.timeout.max(0.0));

    // Real Python's `parse_index()`-based custom scraping has no hook
    // in this port (see this module's own doc) -- always parse_feeds.
    let mut feeds = parse_feeds(hooks, browser);
    feeds = remove_duplicate_articles(cfg, feeds);
    if feeds.is_empty() {
        anyhow::bail!("No articles found, aborting");
    }

    download_feed_images(&mut feeds, browser, timeout, output_dir);

    let cover_path = crate::web::feeds::cover::download_cover(browser, hooks, output_dir, timeout);
    let masthead_path = crate::web::feeds::cover::resolve_masthead(browser, hooks, output_dir, timeout, db);

    if cfg.reverse_article_order {
        for feed in &mut feeds {
            feed.articles.reverse();
        }
    }

    let has_single_feed = feeds.len() == 1;
    let masthead_relname = masthead_path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let date_str = chrono::Utc::now().format("%a, %d %b").to_string();
    let index_html = feeds2index(hooks, &feeds, touchscreen, &masthead_relname, &date_str);
    let index_path = output_dir.join("index.html");
    std::fs::write(&index_path, &index_html)?;

    // Build one job per article, matching real Python's own
    // `WorkRequest` construction loop. Each job borrows directly into
    // `feeds` (`&Feed`/`&Article`) -- this borrow is only ever read by
    // the worker pool, never held across the later point where
    // `feeds` gets mutated (results are applied to `feeds` only after
    // `thread::scope` below returns and this borrow has ended).
    struct Job<'a> {
        feed_index: usize,
        article_index: usize,
        feed_len: usize,
        feed: &'a Feed,
        article: &'a Article,
        art_dir: PathBuf,
    }
    let mut jobs = Vec::new();
    for (f, feed) in feeds.iter().enumerate() {
        let feed_dir = output_dir.join(format!("feed_{f}"));
        std::fs::create_dir_all(&feed_dir)?;
        for (a, article) in feed.articles.iter().enumerate() {
            if a >= cfg.max_articles_per_feed {
                break;
            }
            let art_dir = feed_dir.join(format!("article_{a}"));
            std::fs::create_dir_all(&art_dir)?;
            jobs.push(Job { feed_index: f, article_index: a, feed_len: feed.articles.len(), feed, article, art_dir });
        }
    }

    let image_cache: Mutex<HashMap<String, String>> = Mutex::new(HashMap::new());
    let stylesheet_cache: Mutex<HashMap<String, String>> = Mutex::new(HashMap::new());
    let queue = Mutex::new(jobs.into_iter());
    let num_workers = simultaneous_downloads.max(1);

    let outcomes: Vec<JobOutcome> = thread::scope(|scope| {
        let (tx, rx) = std::sync::mpsc::channel::<JobOutcome>();
        for _ in 0..num_workers {
            let queue = &queue;
            let tx = tx.clone();
            let image_cache = &image_cache;
            let stylesheet_cache = &stylesheet_cache;
            scope.spawn(move || loop {
                let job = { queue.lock().unwrap().next() };
                let Some(job) = job else { break };
                let outcome = match resolve_article_source(hooks, job.feed, job.article, &job.art_dir) {
                    Some((url, preloaded)) => fetch_article(hooks, browser, image_cache, stylesheet_cache, &url, preloaded, &job.art_dir, job.feed_index, job.article_index, job.feed_len, has_single_feed, touchscreen),
                    None => Err("Article has no URL".to_string()),
                };
                if tx.send(JobOutcome { feed_index: job.feed_index, article_index: job.article_index, result: outcome }).is_err() {
                    break;
                }
            });
        }
        drop(tx);
        rx.into_iter().collect()
    });
    drop(queue);

    // `feeds` is no longer borrowed by anything from the worker pool
    // at this point -- safe to mutate.
    for outcome in &outcomes {
        if outcome.result.is_ok() {
            feeds[outcome.feed_index].articles[outcome.article_index].downloaded = true;
        }
        // Real Python records failures in `self.failed_downloads` for
        // `download()`'s own final summary log; a caller here can
        // inspect `outcomes`/`Article::downloaded` directly for the
        // same information.
    }

    for (f, _feed) in feeds.iter().enumerate() {
        let html = feed2index(hooks, f, &feeds, touchscreen);
        let feed_dir = output_dir.join(format!("feed_{f}"));
        std::fs::write(feed_dir.join("index.html"), html)?;
    }

    let cover_relpath = cover_path.as_ref().and_then(|p| p.strip_prefix(output_dir).ok()).map(|p| p.to_string_lossy().into_owned());
    let opf_opts = crate::web::feeds::opf::CreateOpfOptions {
        title: &hooks.short_title(),
        description: &cfg.description,
        language: &cfg.language,
        publication_type: cfg.publication_type.as_deref(),
        pubdate: chrono::Utc::now(),
        summary_length: cfg.summary_length,
        cover_relpath: cover_relpath.as_deref(),
        masthead_relpath: if masthead_relname.is_empty() { None } else { Some(masthead_relname.as_str()) },
    };
    crate::web::feeds::opf::create_opf(output_dir, &feeds, &opf_opts, db)?;

    Ok(index_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::OnceLock;

    struct TestRecipe(RecipeConfig);
    impl NewsRecipeHooks for TestRecipe {
        fn config(&self) -> &RecipeConfig {
            &self.0
        }
    }
    impl NewsRecipePostprocessHooks for TestRecipe {}

    /// A tiny multi-route single-threaded HTTP/1.1 test server --
    /// matches `web::fetch::recursive`'s own private `TestSite`
    /// helper, redefined here since test helpers aren't shared across
    /// modules' private `#[cfg(test)]` blocks.
    struct TestSite {
        addr: std::net::SocketAddr,
    }

    impl TestSite {
        fn start(routes: HashMap<&'static str, (&'static str, &'static [u8])>) -> TestSite {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    let mut stream: TcpStream = stream;
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() {
                        continue;
                    }
                    let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                    loop {
                        let mut l = String::new();
                        if reader.read_line(&mut l).is_err() || l.trim().is_empty() {
                            break;
                        }
                    }
                    match routes.get(path.as_str()) {
                        Some((ctype, body)) => {
                            let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                            let _ = stream.write_all(resp.as_bytes());
                            let _ = stream.write_all(body);
                        }
                        None => {
                            let resp = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                            let _ = stream.write_all(resp.as_bytes());
                        }
                    }
                }
            });
            TestSite { addr }
        }

        fn url(&self, path: &str) -> String {
            format!("http://{}{}", self.addr, path)
        }
    }

    fn test_db() -> &'static Arc<fontdb::Database> {
        static DB: OnceLock<Arc<fontdb::Database>> = OnceLock::new();
        DB.get_or_init(|| {
            let mut db = fontdb::Database::new();
            db.load_system_fonts();
            Arc::new(db)
        })
    }

    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("calibre-oxide-test-download-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn downloaded_article(title: &str, url: &str) -> Article {
        let mut a = Article::new("id", Some(title), Some(url.to_string()), None, Some("Summary".to_string()), None, None);
        a.downloaded = true;
        a
    }

    // ===============================================================
    // parse_feeds
    // ===============================================================

    #[test]
    fn parse_feeds_returns_empty_when_get_feeds_is_not_implemented() {
        let hooks = TestRecipe(RecipeConfig::default());
        let browser = Browser::new("", &[], true);
        assert!(parse_feeds(&hooks, &browser).is_empty());
    }

    #[test]
    fn parse_feeds_downloads_and_parses_a_real_feed() {
        let rss = br#"<?xml version="1.0"?>
<rss version="2.0"><channel><title>Test Feed</title><description>d</description>
<item><title>Article One</title><link>http://example.com/1</link><description>Summary</description></item>
</channel></rss>"#;
        let mut routes = HashMap::new();
        routes.insert("/feed.xml", ("application/rss+xml", rss.as_slice()));
        let site = TestSite::start(routes);

        let cfg = RecipeConfig { feeds: Some(vec![(Some("My Feed".to_string()), site.url("/feed.xml"))]), ..Default::default() };
        let hooks = TestRecipe(cfg);
        let browser = Browser::new("", &[], true);

        let feeds = parse_feeds(&hooks, &browser);
        assert_eq!(feeds.len(), 1);
        assert_eq!(feeds[0].articles.len(), 1);
        assert_eq!(feeds[0].articles[0].title, "Article One");
    }

    #[test]
    fn parse_feeds_produces_an_error_placeholder_feed_on_fetch_failure() {
        let cfg = RecipeConfig { feeds: Some(vec![(Some("Broken".to_string()), "http://127.0.0.1:1/nope".to_string())]), ..Default::default() };
        let hooks = TestRecipe(cfg);
        let browser = Browser::new("", &[], true);

        let feeds = parse_feeds(&hooks, &browser);
        assert_eq!(feeds.len(), 1, "a failed feed still produces a placeholder, not a crash");
        assert!(feeds[0].articles.is_empty());
    }

    // ===============================================================
    // remove_duplicate_articles
    // ===============================================================

    #[test]
    fn remove_duplicate_articles_dedupes_by_url_across_feeds() {
        let mut feed_a = Feed::populate_from_preparsed_feed(Some("A"), &[], 36500.0, 10);
        feed_a.articles = vec![downloaded_article("One", "http://x/1")];
        let mut feed_b = Feed::populate_from_preparsed_feed(Some("B"), &[], 36500.0, 10);
        feed_b.articles = vec![downloaded_article("One Again", "http://x/1"), downloaded_article("Two", "http://x/2")];

        let cfg = RecipeConfig { ignore_duplicate_articles_by_url: true, ..Default::default() };
        let feeds = remove_duplicate_articles(&cfg, vec![feed_a, feed_b]);

        assert_eq!(feeds[0].articles.len(), 1);
        assert_eq!(feeds[1].articles.len(), 1, "the second feed's duplicate URL must be removed");
        assert_eq!(feeds[1].articles[0].title, "Two");
    }

    #[test]
    fn remove_duplicate_articles_is_a_noop_when_disabled() {
        let mut feed = Feed::populate_from_preparsed_feed(Some("A"), &[], 36500.0, 10);
        feed.articles = vec![downloaded_article("One", "http://x/1"), downloaded_article("One", "http://x/1")];
        let feeds = remove_duplicate_articles(&RecipeConfig::default(), vec![feed]);
        assert_eq!(feeds[0].articles.len(), 2);
    }

    // ===============================================================
    // feeds2index / feed2index / lang_for_html
    // ===============================================================

    #[test]
    fn lang_for_html_strips_region_and_treats_und_as_none() {
        assert_eq!(lang_for_html("en_US"), Some("en".to_string()));
        assert_eq!(lang_for_html("und"), None);
        assert_eq!(lang_for_html(""), None);
    }

    #[test]
    fn feeds2index_lists_every_feed() {
        let mut feed = Feed::populate_from_preparsed_feed(Some("Feed A"), &[], 36500.0, 10);
        feed.articles = vec![downloaded_article("Art", "http://x/1")];
        let hooks = TestRecipe(RecipeConfig { title: "My Paper".to_string(), ..Default::default() });
        let html = feeds2index(&hooks, &[feed], false, "masthead.jpg", "Mon, 01 Jan");
        assert!(html.contains("Feed A"));
        assert!(html.contains("My Paper"));
    }

    #[test]
    fn feed2index_renders_the_articles_in_one_feed() {
        let mut feed = Feed::populate_from_preparsed_feed(Some("Feed A"), &[], 36500.0, 10);
        feed.articles = vec![downloaded_article("Art One", "http://x/1")];
        let hooks = TestRecipe(RecipeConfig::default());
        let html = feed2index(&hooks, 0, &[feed], false);
        assert!(html.contains("Art One"));
    }

    // ===============================================================
    // resolve_article_source
    // ===============================================================

    #[test]
    fn resolve_article_source_uses_the_plain_url_by_default() {
        let feed = Feed::populate_from_preparsed_feed(Some("F"), &[], 36500.0, 10);
        let article = downloaded_article("A", "http://example.com/a");
        let hooks = TestRecipe(RecipeConfig::default());
        let dir = test_dir("resolve-plain");

        let (url, preloaded) = resolve_article_source(&hooks, &feed, &article, &dir).unwrap();
        assert_eq!(url, "http://example.com/a");
        assert!(preloaded.is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_article_source_uses_embedded_content_when_the_feed_has_it() {
        let mut feed = Feed::populate_from_preparsed_feed(Some("F"), &[], 36500.0, 10);
        // `Feed::has_embedded_content` requires total content/summary
        // length > 2000 * article_count -- pad well past that.
        let long_content = format!("<p>{}</p>", "Full article body text. ".repeat(200));
        let mut article = Article::new("id", Some("A"), Some("http://example.com/a".to_string()), None, None, None, None);
        article.content = Some(long_content);
        feed.articles = vec![article.clone()];
        let hooks = TestRecipe(RecipeConfig::default());
        let dir = test_dir("resolve-embedded");

        let (url, preloaded) = resolve_article_source(&hooks, &feed, &article, &dir).unwrap();
        assert!(url.starts_with("file://"), "{url}");
        assert!(preloaded.is_none());
        let path = url.strip_prefix("file://").unwrap();
        assert!(std::path::Path::new(path).is_file());

        std::fs::remove_dir_all(&dir).ok();
    }

    // ===============================================================
    // build_index: end-to-end
    // ===============================================================

    #[test]
    fn build_index_downloads_a_real_small_periodical_end_to_end() {
        let rss = br#"<?xml version="1.0"?>
<rss version="2.0"><channel><title>Test Feed</title><description>d</description>
<item><title>Article One</title><link>/article1</link><description>Summary one</description></item>
<item><title>Article Two</title><link>/article2</link><description>Summary two</description></item>
</channel></rss>"#;
        let mut routes = HashMap::new();
        routes.insert("/feed.xml", ("application/rss+xml", rss.as_slice()));
        routes.insert("/article1", ("text/html", b"<html><body><p>Full text of article one.</p></body></html>".as_slice()));
        routes.insert("/article2", ("text/html", b"<html><body><p>Full text of article two.</p></body></html>".as_slice()));
        let site = TestSite::start(routes);

        let cfg = RecipeConfig { title: "My Weekly".to_string(), feeds: Some(vec![(None, site.url("/feed.xml"))]), ..Default::default() };
        let hooks = TestRecipe(cfg);
        let browser = Browser::new("", &[], true);
        let dir = test_dir("build-index-e2e");

        let index_path = build_index(&hooks, &browser, &dir, false, 2, test_db()).expect("a small real periodical should download end to end");

        assert!(index_path.is_file());
        let index_html = std::fs::read_to_string(&index_path).unwrap();
        assert!(index_html.contains("Test Feed"), "{index_html}");

        assert!(dir.join("feed_0/index.html").is_file());
        assert!(dir.join("feed_0/article_0").is_dir());
        assert!(dir.join("feed_0/article_1").is_dir());
        assert!(dir.join("index.opf").is_file());
        assert!(dir.join("index.ncx").is_file());

        let opf = std::fs::read_to_string(dir.join("index.opf")).unwrap();
        assert!(opf.contains("My Weekly"), "{opf}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn build_index_errors_when_no_articles_are_found() {
        let hooks = TestRecipe(RecipeConfig::default()); // no feeds configured at all
        let browser = Browser::new("", &[], true);
        let dir = test_dir("build-index-empty");

        let result = build_index(&hooks, &browser, &dir, false, 1, test_db());
        assert!(result.is_err());

        std::fs::remove_dir_all(&dir).ok();
    }
}
