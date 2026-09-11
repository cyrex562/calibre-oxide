//! Port of `RecursiveFetcher`'s recursive crawl engine itself
//! (`old_src/src/calibre/web/fetch/simple.py`, issue #633, the final
//! piece of the #455 epic split -- see `docs/modules_to_port.md`'s
//! `simple.py` entry for the full split rationale): `start_fetch`/
//! `process_links`/`process_return_links`/`save_soup`, tying #630
//! (fetch + link filtering), #631 (HTML preprocessing + tag matching)
//! and #632 (image/stylesheet download) together.
//!
//! # Shape: context + explicit state, not `&mut self` methods
//!
//! [`FetcherContext`] holds everything that doesn't change during one
//! crawl (`hooks`, `browser`, a per-instance [`simple::FetchThrottle`]/
//! preloaded-URL map, and -- crucially -- **shared, externally-owned**
//! `image_cache`/`stylesheet_cache` references: real Python constructs
//! multiple `RecursiveFetcher` instances, one per article, sharing the
//! *same* `imagemap`/`stylemap` dicts under a lock so dedup works
//! across concurrently-fetched sibling articles (#623's job to spin up
//! N of these; this module just needs the caches to already be
//! shareable). [`FetchState`] holds the actually-mutated-during-
//! recursion state (`current_dir`, `filemap`, counters). Threading
//! both explicitly through free functions (rather than a single
//! `&mut self` struct) sidesteps a real Rust-vs-Python difference:
//! Python's `self.current_dir = X; ...; self.current_dir = prev` save/
//! restore dance around a recursive call has no borrow-checker
//! friction to work around, but the equivalent in Rust is far simpler
//! to write as separate `ctx`/`state` parameters than as recursive
//! `&mut self` calls that would otherwise need to juggle borrows of
//! `self.hooks`/`self.browser` alongside `&mut self.current_dir`.
//!
//! # Disclosed: `AbortArticle` has no reachable raise path here
//!
//! Real Python's `process_links` re-raises `AbortArticle` specially
//! (propagating it all the way up rather than recording it as a
//! failed link) if a hook raises one. In this port,
//! [`NewsRecipePostprocessHooks::abort_article`] (#620) is a plain
//! constructor helper returning an `AbortArticle` value -- but every
//! hook that could conceivably call it (`preprocess_html`/
//! `postprocess_html`) returns a plain `Dom`, not a `Result`, so there
//! is no path by which one could actually surface here. This is a
//! narrowing already baked into #620's own hook shapes, not something
//! newly introduced by this issue -- every per-link failure is
//! recorded in `failed_links`, matching real Python's *other*
//! exception branch (`except Exception`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use crate::dom::{Dom, NodeId};
use crate::scraper::Browser;
use crate::web::feeds::postprocess::{self, NavbarContext, NewsRecipePostprocessHooks};
use crate::web::feeds::recipe::NewsRecipeHooks;
use crate::web::fetch::get_soup;
use crate::web::fetch::media::{self, ImageCompressionOptions};
use crate::web::fetch::simple::{self, FetchError, FetchThrottle, FetchedResource};

/// Owned mirror of [`NavbarContext`] (which borrows its `url`) --
/// real Python's `job_info`, a fixed `(url, feed_index, article_index,
/// feed_len)` tuple set once per `RecursiveFetcher` instance (one
/// instance per article) and read on every first-fetch call.
#[derive(Debug, Clone)]
pub struct OwnedNavbarContext {
    pub url: String,
    pub feed_index: usize,
    pub article_index: usize,
    pub feed_len: usize,
    pub has_single_feed: bool,
}

impl OwnedNavbarContext {
    fn as_context(&self) -> NavbarContext<'_> {
        NavbarContext { url: &self.url, feed_index: self.feed_index, article_index: self.article_index, feed_len: self.feed_len, has_single_feed: self.has_single_feed }
    }
}

/// The real `RecursiveFetcher.__init__` config surface this module
/// needs. `max_files` defaults effectively unbounded
/// (`usize::MAX`) -- real Python's own CLI-only default
/// (`sys.maxsize`), never overridden by `news.py`'s real call sites.
#[derive(Debug, Clone)]
pub struct FetcherConfig {
    pub timeout: Duration,
    pub max_recursions: u32,
    pub max_files: usize,
    pub match_regexps: Vec<regex::Regex>,
    pub filter_regexps: Vec<regex::Regex>,
    pub preprocess_regexps: Vec<(regex::Regex, String)>,
    pub download_stylesheets: bool,
    /// Port of `self.encoding`: `None` is real Python's own default
    /// (`getattr(options, 'encoding', ...)` -> `BasicNewsRecipe.encoding
    /// = None`), auto-detected via #631's `get_soup` (which already
    /// runs `xml_to_unicode` internally). `Some(codec)` decodes with a
    /// fixed named codec first, matching Python's `dsrc.decode(encoding,
    /// 'replace')` branch. Disclosed narrowing: a *callable* encoding
    /// override has no equivalent (no hook models it, and no real
    /// recipe in this port's own scope uses one).
    pub encoding: Option<String>,
    pub compression: ImageCompressionOptions,
    pub touchscreen: bool,
}

impl Default for FetcherConfig {
    fn default() -> Self {
        FetcherConfig { timeout: Duration::from_secs(120), max_recursions: 0, max_files: usize::MAX, match_regexps: Vec::new(), filter_regexps: Vec::new(), preprocess_regexps: Vec::new(), download_stylesheets: true, encoding: None, compression: ImageCompressionOptions::default(), touchscreen: false }
    }
}

/// Everything a crawl needs that doesn't change once fetching starts.
/// `image_cache`/`stylesheet_cache` are borrowed, not owned, so
/// multiple `FetcherContext`s (one per article, #623's job) can share
/// the same underlying maps.
pub struct FetcherContext<'a, H> {
    pub hooks: &'a H,
    pub browser: &'a Browser,
    pub throttle: FetchThrottle,
    pub preloaded: Mutex<HashMap<String, Vec<u8>>>,
    pub image_cache: &'a Mutex<HashMap<String, String>>,
    pub stylesheet_cache: &'a Mutex<HashMap<String, String>>,
    pub config: FetcherConfig,
    pub job_info: Option<OwnedNavbarContext>,
}

impl<'a, H> FetcherContext<'a, H> {
    pub fn new(hooks: &'a H, browser: &'a Browser, image_cache: &'a Mutex<HashMap<String, String>>, stylesheet_cache: &'a Mutex<HashMap<String, String>>, config: FetcherConfig, job_info: Option<OwnedNavbarContext>) -> Self {
        FetcherContext { hooks, browser, throttle: FetchThrottle::new(), preloaded: Mutex::new(HashMap::new()), image_cache, stylesheet_cache, config, job_info }
    }
}

/// The mutable, per-crawl state real Python keeps on `self`
/// (`current_dir`, `filemap`, `files`, `downloaded_paths`,
/// `failed_links`, `called_first`).
#[derive(Debug, Clone)]
pub struct FetchState {
    pub current_dir: PathBuf,
    pub filemap: HashMap<String, String>,
    pub files: usize,
    pub downloaded_paths: Vec<PathBuf>,
    pub failed_links: Vec<(String, String)>,
    called_first: bool,
}

impl FetchState {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        FetchState { current_dir: base_dir.into(), filemap: HashMap::new(), files: 0, downloaded_paths: Vec::new(), failed_links: Vec::new(), called_first: false }
    }
}

fn make_get_delay<H: NewsRecipeHooks>(hooks: &H) -> impl Fn(&str) -> Duration + '_ {
    move |u| Duration::from_secs_f64(hooks.get_url_specific_delay(u).max(0.0))
}

fn make_fetcher<'a, H: NewsRecipeHooks>(ctx: &'a FetcherContext<'a, H>) -> impl Fn(&str) -> Result<FetchedResource, FetchError> + 'a {
    move |u| simple::fetch_url(ctx.browser, &ctx.throttle, &ctx.preloaded, u, ctx.config.timeout, make_get_delay(ctx.hooks))
}

fn strip_html_comments(data: &[u8]) -> Vec<u8> {
    static RE: std::sync::OnceLock<regex::bytes::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::bytes::Regex::new(r"(?s)<!--.*?-->").expect("static pattern"));
    re.replace_all(data, &b""[..]).into_owned()
}

/// Port of `self.encoding is not None: dsrc.decode(self.encoding, 'replace')`,
/// re-encoded to UTF-8 bytes since #631's `get_soup` (which runs its
/// own `xml_to_unicode` auto-detection) always takes bytes.
fn decode_fixed_codec_as_utf8(data: &[u8], codec: &str) -> Vec<u8> {
    match encoding_rs::Encoding::for_label(codec.as_bytes()) {
        Some(enc) => {
            let (text, _, _) = enc.decode(data);
            text.into_owned().into_bytes()
        }
        None => data.to_vec(),
    }
}

fn build_link_filename(iurl: &str) -> String {
    let mut fname = simple::basename(iurl);
    fname = fname.replace('%', "").replace(std::path::MAIN_SEPARATOR, "");
    fname = calibre_utils::filenames::ascii_filename(&fname);
    let stem = fname.rsplit_once('.').map(|(s, _)| s).unwrap_or(fname.as_str());
    let truncated: String = stem.chars().take(120).collect();
    format!("{truncated}.xhtml")
}

fn rewrite_root_relative_hrefs(dom: &mut Dom, base_url: &str) {
    let candidates: Vec<NodeId> = dom.find_all_tag_global("a").into_iter().filter(|&n| dom.node(n).attrs.get("href").is_some_and(|h| h.starts_with('/'))).collect();
    for id in candidates {
        let Some(href) = dom.node(id).attrs.get("href").cloned() else { continue };
        if let Ok(joined) = url::Url::parse(base_url).and_then(|b| b.join(&href)) {
            dom.node_mut(id).attrs.insert("href".to_string(), joined.to_string());
        }
    }
}

/// Port of the module-level `save_soup` function: strips any
/// charset-declaring `<meta>`, inserts a fresh `<meta charset="utf-8">`,
/// rewrites any `img`/`link`/`a` `src`/`href` that's currently an
/// absolute path to an existing file into a path relative to
/// `target`'s own directory (via `pathdiff`, matching real Python's
/// `relpath`), then serializes and writes.
fn save_soup(dom: &mut Dom, target: &Path) -> std::io::Result<()> {
    let meta_ids: Vec<NodeId> = dom
        .find_all_tag_global("meta")
        .into_iter()
        .filter(|&id| {
            let attrs = &dom.node(id).attrs;
            attrs.get("content").is_some_and(|c| c.to_lowercase().contains("charset")) || attrs.contains_key("charset")
        })
        .collect();
    for id in meta_ids {
        dom.detach(id);
    }
    if let Some(head) = dom.find_first_tag_global("head") {
        let meta = dom.new_element("meta");
        dom.node_mut(meta).attrs.insert("charset".to_string(), "utf-8".to_string());
        dom.insert_child(head, 0, meta);
    }

    let selfdir = target.parent().unwrap_or_else(|| Path::new("."));
    for tag_name in ["img", "link", "a"] {
        for id in dom.find_all_tag_global(tag_name) {
            for key in ["src", "href"] {
                let Some(path_str) = dom.node(id).attrs.get(key).cloned() else { continue };
                let p = Path::new(&path_str);
                if p.is_absolute() && p.is_file() {
                    if let Some(rel) = pathdiff::diff_paths(p, selfdir) {
                        let rel_str = rel.to_string_lossy().replace('\\', "/");
                        dom.node_mut(id).attrs.insert(key.to_string(), rel_str);
                    }
                }
            }
        }
    }

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(target, dom.serialize(dom.root).as_bytes())
}

/// Port of `RecursiveFetcher.process_return_links`.
fn process_return_links<H: NewsRecipePostprocessHooks>(ctx: &FetcherContext<H>, state: &FetchState, soup: &mut Dom, base_url: &str) {
    let tags: Vec<NodeId> = soup.find_all_tag_global("a").into_iter().filter(|&n| soup.node(n).attrs.contains_key("href")).collect();
    for tag in tags {
        let href = soup.node(tag).attrs.get("href").cloned().unwrap_or_default();
        let Some(iurl) = simple::absurl(base_url, &href, true, |u| simple::is_link_wanted(u, ctx.hooks.is_link_wanted(u, "a"), &ctx.config.filter_regexps, &ctx.config.match_regexps)) else { continue };
        let nurl = simple::normurl(&iurl);
        if let Some(saved) = state.filemap.get(&nurl) {
            let new_href = simple::localize_link(&href, saved);
            soup.node_mut(tag).attrs.insert("href".to_string(), new_href);
        }
    }
}

/// Port of `RecursiveFetcher.process_links`.
fn process_links<H: NewsRecipePostprocessHooks>(ctx: &FetcherContext<H>, state: &mut FetchState, soup: &mut Dom, base_url: &str, recursion_level: u32, into_dir: &str) -> Option<String> {
    let mut res: Option<String> = None;
    let diskpath = if into_dir.is_empty() { state.current_dir.clone() } else { state.current_dir.join(into_dir) };
    let _ = std::fs::create_dir_all(&diskpath);
    let prev_dir = std::mem::replace(&mut state.current_dir, diskpath.clone());

    let filter = recursion_level != 0;
    let tags: Vec<NodeId> = soup.find_all_tag_global("a").into_iter().filter(|&n| soup.node(n).attrs.contains_key("href")).collect();

    for (c, &tag) in tags.iter().enumerate() {
        let href = soup.node(tag).attrs.get("href").cloned().unwrap_or_default();
        let iurl = simple::absurl(base_url, &href, filter, |u| simple::is_link_wanted(u, ctx.hooks.is_link_wanted(u, "a"), &ctx.config.filter_regexps, &ctx.config.match_regexps));
        let Some(iurl) = iurl else { continue };

        let nurl = simple::normurl(&iurl);
        if let Some(saved) = state.filemap.get(&nurl).cloned() {
            let new_href = simple::localize_link(&href, &saved);
            soup.node_mut(tag).attrs.insert("href".to_string(), new_href);
            continue;
        }
        if state.files > ctx.config.max_files {
            state.current_dir = prev_dir;
            return res;
        }

        let linkdir = if !into_dir.is_empty() { format!("link{c}") } else { String::new() };
        let linkdiskpath = if linkdir.is_empty() { diskpath.clone() } else { diskpath.join(&linkdir) };
        let _ = std::fs::create_dir_all(&linkdiskpath);
        state.current_dir = linkdiskpath;

        let outcome = fetch_and_process_one_link(ctx, state, &iurl, recursion_level, c);

        state.current_dir = diskpath.clone();
        state.files += 1;

        match outcome {
            Ok(saved_path) => {
                state.filemap.insert(nurl, saved_path.clone());
                let new_href = simple::localize_link(&href, &saved_path);
                soup.node_mut(tag).attrs.insert("href".to_string(), new_href);
                res = Some(saved_path);
            }
            Err(msg) => {
                state.failed_links.push((iurl, msg));
            }
        }
    }

    state.current_dir = prev_dir;
    res
}

fn fetch_and_process_one_link<H: NewsRecipePostprocessHooks>(ctx: &FetcherContext<H>, state: &mut FetchState, iurl: &str, recursion_level: u32, c: usize) -> Result<String, String> {
    let resource = make_fetcher(ctx)(iurl).map_err(|e| e.to_string())?;
    let mut newbaseurl = resource.new_url.clone().unwrap_or_else(|| iurl.to_string());
    let raw = resource.data;

    if raw.is_empty() || strip_html_comments(&raw).iter().all(u8::is_ascii_whitespace) {
        return Err(format!("No content at URL {iurl:?}"));
    }

    let decoded: Vec<u8> = match &ctx.config.encoding {
        Some(codec) => decode_fixed_codec_as_utf8(&raw, codec),
        None => raw,
    };

    let mut page = get_soup::get_soup(ctx.hooks, &decoded, Some(iurl), &ctx.config.preprocess_regexps);

    if let Some(href) = page.find_first_tag_global("base").and_then(|id| page.node(id).attrs.get("href").cloned()) {
        newbaseurl = href;
    }

    let images_dir = state.current_dir.join("images");
    media::process_images(&mut page, &newbaseurl, &images_dir, ctx.image_cache, make_fetcher(ctx), |b, u| ctx.hooks.image_url_processor(b, u), |d, u| ctx.hooks.preprocess_image(d, u), &ctx.config.compression).map_err(|e| e.to_string())?;

    if ctx.config.download_stylesheets {
        let styles_dir = state.current_dir.join("stylesheets");
        media::process_stylesheets(&mut page, &newbaseurl, &styles_dir, ctx.stylesheet_cache, make_fetcher(ctx)).map_err(|e| e.to_string())?;
    }

    let fname = build_link_filename(iurl);
    let saved_path = state.current_dir.join(&fname);
    state.downloaded_paths.push(saved_path.clone());

    if recursion_level < ctx.config.max_recursions {
        process_links(ctx, state, &mut page, &newbaseurl, recursion_level + 1, "links");
    } else {
        process_return_links(ctx, state, &mut page, &newbaseurl);
    }

    if !newbaseurl.is_empty() && !newbaseurl.starts_with('/') {
        rewrite_root_relative_hrefs(&mut page, &newbaseurl);
    }

    let first_fetch = c == 0 && recursion_level == 0 && !state.called_first;
    let navbar_ctx = ctx.job_info.as_ref().map(OwnedNavbarContext::as_context);
    let mut page = postprocess::postprocess_html(ctx.hooks, page, first_fetch, navbar_ctx, ctx.config.touchscreen);
    if c == 0 && recursion_level == 0 {
        state.called_first = true;
    }

    save_soup(&mut page, &saved_path).map_err(|e| e.to_string())?;
    Ok(saved_path.to_string_lossy().into_owned())
}

/// Port of `RecursiveFetcher.start_fetch`: wraps `url` in a synthetic
/// single-link page and kicks off `process_links` at recursion level
/// 0 (with `into_dir=""`, matching real Python's own top-level call --
/// the very first fetch is written directly into `base_dir`, not a
/// `links/link0/` subdirectory).
pub fn start_fetch<H: NewsRecipePostprocessHooks>(ctx: &FetcherContext<H>, state: &mut FetchState, url: &str) -> Option<String> {
    let mut synthetic = Dom::empty();
    let a = synthetic.new_element("a");
    synthetic.node_mut(a).attrs.insert("href".to_string(), url.to_string());
    let root = synthetic.root;
    synthetic.append_child(root, a);
    process_links(ctx, state, &mut synthetic, url, 0, "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::feeds::recipe::RecipeConfig;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::Arc;
    use std::thread;

    struct TestRecipe(RecipeConfig);
    impl NewsRecipeHooks for TestRecipe {
        fn config(&self) -> &RecipeConfig {
            &self.0
        }
    }
    impl NewsRecipePostprocessHooks for TestRecipe {}

    /// A tiny multi-route single-threaded HTTP/1.1 test server: serves
    /// whatever `(status, content_type, body)` a route was registered
    /// with, tracking a hit-count per path.
    struct TestSite {
        addr: std::net::SocketAddr,
        hits: Arc<Mutex<HashMap<String, usize>>>,
    }

    impl TestSite {
        fn start(routes: HashMap<&'static str, (&'static str, &'static [u8])>) -> TestSite {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let hits = Arc::new(Mutex::new(HashMap::new()));
            let hits2 = Arc::clone(&hits);
            thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    let mut stream = stream;
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
                    *hits2.lock().unwrap().entry(path.clone()).or_insert(0) += 1;
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
            TestSite { addr, hits }
        }

        fn url(&self, path: &str) -> String {
            format!("http://{}{}", self.addr, path)
        }

        fn hit_count(&self, path: &str) -> usize {
            self.hits.lock().unwrap().get(path).copied().unwrap_or(0)
        }
    }

    fn ctx_for<'a>(hooks: &'a TestRecipe, browser: &'a Browser, image_cache: &'a Mutex<HashMap<String, String>>, stylesheet_cache: &'a Mutex<HashMap<String, String>>, config: FetcherConfig) -> FetcherContext<'a, TestRecipe> {
        FetcherContext::new(hooks, browser, image_cache, stylesheet_cache, config, None)
    }

    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("calibre-oxide-test-recursive-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn start_fetch_saves_the_top_level_page_directly_into_base_dir() {
        let mut routes = HashMap::new();
        routes.insert("/", ("text/html", b"<html><body><p>Hello</p></body></html>".as_slice()));
        let site = TestSite::start(routes);

        let hooks = TestRecipe(RecipeConfig::default());
        let browser = Browser::new("", &[], true);
        let image_cache = Mutex::new(HashMap::new());
        let style_cache = Mutex::new(HashMap::new());
        let ctx = ctx_for(&hooks, &browser, &image_cache, &style_cache, FetcherConfig::default());
        let dir = test_dir("toplevel");
        let mut state = FetchState::new(&dir);

        let saved = start_fetch(&ctx, &mut state, &site.url("/"));
        let saved = saved.expect("the top-level fetch should succeed");
        assert!(Path::new(&saved).is_file());
        // The very first fetch writes directly into base_dir, not a
        // "links/link0/" subdirectory.
        assert_eq!(Path::new(&saved).parent().unwrap(), dir.as_path());
        assert!(std::fs::read_to_string(&saved).unwrap().contains("Hello"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn process_links_respects_max_recursions_depth_limit() {
        let mut routes = HashMap::new();
        routes.insert("/page1", ("text/html", b"<html><body><a href=\"/page2\">next</a></body></html>".as_slice()));
        routes.insert("/page2", ("text/html", b"<html><body><a href=\"/page3\">next</a></body></html>".as_slice()));
        routes.insert("/page3", ("text/html", b"<html><body>leaf</body></html>".as_slice()));
        let site = TestSite::start(routes);

        let hooks = TestRecipe(RecipeConfig::default());
        let browser = Browser::new("", &[], true);
        let image_cache = Mutex::new(HashMap::new());
        let style_cache = Mutex::new(HashMap::new());
        let mut config = FetcherConfig::default();
        config.max_recursions = 1; // follow one level of links beyond the start page
        let ctx = ctx_for(&hooks, &browser, &image_cache, &style_cache, config);
        let dir = test_dir("depth-limit");
        let mut state = FetchState::new(&dir);

        start_fetch(&ctx, &mut state, &site.url("/page1"));

        // page1 (level 0) is fetched by start_fetch itself; its own
        // link to page2 is followed at level 1 (<= max_recursions);
        // page2's link to page3 would be level 2, beyond the limit, so
        // page3 must never be fetched.
        assert_eq!(site.hit_count("/page1"), 1);
        assert_eq!(site.hit_count("/page2"), 1);
        assert_eq!(site.hit_count("/page3"), 0, "page3 is beyond max_recursions and must not be fetched");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn process_links_localizes_a_link_to_an_already_downloaded_page() {
        let mut routes = HashMap::new();
        routes.insert("/start", ("text/html", b"<html><body><a href=\"/a\">a</a><a href=\"/a\">a again</a></body></html>".as_slice()));
        routes.insert("/a", ("text/html", b"<html><body>A</body></html>".as_slice()));
        let site = TestSite::start(routes);

        let hooks = TestRecipe(RecipeConfig::default());
        let browser = Browser::new("", &[], true);
        let image_cache = Mutex::new(HashMap::new());
        let style_cache = Mutex::new(HashMap::new());
        let mut config = FetcherConfig::default();
        config.max_recursions = 1;
        let ctx = ctx_for(&hooks, &browser, &image_cache, &style_cache, config);
        let dir = test_dir("localize-repeat");
        let mut state = FetchState::new(&dir);

        start_fetch(&ctx, &mut state, &site.url("/start"));

        // "/a" is linked twice from the start page; it must only be
        // fetched once, with the second reference localized from the
        // filemap cache instead of re-fetched.
        assert_eq!(site.hit_count("/a"), 1, "a repeated link within the same page must be fetched only once");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn process_images_and_stylesheets_are_wired_into_the_crawl() {
        let mut routes = HashMap::new();
        routes.insert("/start", ("text/html", b"<html><head><link rel=\"stylesheet\" type=\"text/css\" href=\"/style.css\"></head><body><img src=\"/pic.png\"></body></html>".as_slice()));
        routes.insert("/style.css", ("text/css", b"body{color:red}".as_slice()));
        let png = image_bytes();
        let mut routes2 = routes;
        // Insert PNG bytes leaked for the closure's lifetime via Box::leak,
        // since TestSite's route map borrows 'static byte slices.
        let leaked: &'static [u8] = Box::leak(png.into_boxed_slice());
        routes2.insert("/pic.png", ("image/png", leaked));
        let site = TestSite::start(routes2);

        let hooks = TestRecipe(RecipeConfig::default());
        let browser = Browser::new("", &[], true);
        let image_cache = Mutex::new(HashMap::new());
        let style_cache = Mutex::new(HashMap::new());
        let ctx = ctx_for(&hooks, &browser, &image_cache, &style_cache, FetcherConfig::default());
        let dir = test_dir("media-wiring");
        let mut state = FetchState::new(&dir);

        let saved = start_fetch(&ctx, &mut state, &site.url("/start")).expect("fetch should succeed");
        let html = std::fs::read_to_string(&saved).unwrap();
        assert!(html.contains("images/img1.png"), "{html}");
        // The `<link>` element itself is unconditionally stripped by
        // `postprocess_html` (real Python's own BAD_TAGS removal --
        // "link tags can be used for preloading causing network
        // activity in calibre viewer") regardless of `no_stylesheets`,
        // so it's real and correct for it to be gone from the saved
        // HTML; what matters is that the stylesheet was actually
        // fetched and cached before that removal happened.
        assert!(!html.contains("<link"), "{html}");
        assert_eq!(image_cache.lock().unwrap().len(), 1);
        assert_eq!(style_cache.lock().unwrap().len(), 1);
        let cached_style_path = style_cache.lock().unwrap().values().next().unwrap().clone();
        assert!(cached_style_path.ends_with("style0.css"), "{cached_style_path}");
        assert!(Path::new(&cached_style_path).is_file());
        assert_eq!(std::fs::read_to_string(&cached_style_path).unwrap(), "body{color:red}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn shared_caches_dedup_across_two_separate_fetcher_contexts() {
        let png = image_bytes();
        let leaked: &'static [u8] = Box::leak(png.into_boxed_slice());
        let mut routes = HashMap::new();
        routes.insert("/page-a", ("text/html", b"<html><body><img src=\"/shared.png\"></body></html>".as_slice()));
        routes.insert("/page-b", ("text/html", b"<html><body><img src=\"/shared.png\"></body></html>".as_slice()));
        routes.insert("/shared.png", ("image/png", leaked));
        let site = TestSite::start(routes);

        let hooks = TestRecipe(RecipeConfig::default());
        let browser = Browser::new("", &[], true);
        // One pair of caches, shared across two independent contexts --
        // matching real Python's own multiple-RecursiveFetcher-
        // instances-sharing-imagemap/stylemap design (#623's job to
        // spin up N of these; this test proves the sharing itself
        // works).
        let image_cache = Mutex::new(HashMap::new());
        let style_cache = Mutex::new(HashMap::new());

        let dir_a = test_dir("shared-cache-a");
        let ctx_a = ctx_for(&hooks, &browser, &image_cache, &style_cache, FetcherConfig::default());
        let mut state_a = FetchState::new(&dir_a);
        start_fetch(&ctx_a, &mut state_a, &site.url("/page-a"));

        let dir_b = test_dir("shared-cache-b");
        let ctx_b = ctx_for(&hooks, &browser, &image_cache, &style_cache, FetcherConfig::default());
        let mut state_b = FetchState::new(&dir_b);
        start_fetch(&ctx_b, &mut state_b, &site.url("/page-b"));

        assert_eq!(site.hit_count("/shared.png"), 1, "the second fetcher's identical image URL must hit the shared cache, not the network");
        assert_eq!(image_cache.lock().unwrap().len(), 1);

        std::fs::remove_dir_all(&dir_a).ok();
        std::fs::remove_dir_all(&dir_b).ok();
    }

    #[test]
    fn max_files_stops_fetching_further_links() {
        let mut routes = HashMap::new();
        routes.insert("/start", ("text/html", b"<html><body><a href=\"/l1\">1</a><a href=\"/l2\">2</a><a href=\"/l3\">3</a></body></html>".as_slice()));
        routes.insert("/l1", ("text/html", b"<html><body>one</body></html>".as_slice()));
        routes.insert("/l2", ("text/html", b"<html><body>two</body></html>".as_slice()));
        routes.insert("/l3", ("text/html", b"<html><body>three</body></html>".as_slice()));
        let site = TestSite::start(routes);

        let hooks = TestRecipe(RecipeConfig::default());
        let browser = Browser::new("", &[], true);
        let image_cache = Mutex::new(HashMap::new());
        let style_cache = Mutex::new(HashMap::new());
        let mut config = FetcherConfig::default();
        config.max_recursions = 1;
        config.max_files = 1;
        let ctx = ctx_for(&hooks, &browser, &image_cache, &style_cache, config);
        let dir = test_dir("max-files");
        let mut state = FetchState::new(&dir);

        start_fetch(&ctx, &mut state, &site.url("/start"));

        // start page itself (files=0 before it's counted... it isn't
        // counted at all, since start_fetch's own into_dir="" call
        // doesn't go through the max_files check path the same way --
        // only l1/l2/l3 count against the limit) -- l1 gets fetched,
        // then files=1 > max_files=1 is false yet (1 > 1 is false), so
        // l2 is also allowed, then files=2 > 1 stops l3.
        assert!(site.hit_count("/l3") == 0, "max_files should cut off the link loop before every link is fetched");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_content_is_recorded_as_a_failed_link_not_a_panic() {
        let mut routes = HashMap::new();
        routes.insert("/start", ("text/html", b"<html><body><a href=\"/empty\">e</a></body></html>".as_slice()));
        routes.insert("/empty", ("text/html", b"".as_slice()));
        let site = TestSite::start(routes);

        let hooks = TestRecipe(RecipeConfig::default());
        let browser = Browser::new("", &[], true);
        let image_cache = Mutex::new(HashMap::new());
        let style_cache = Mutex::new(HashMap::new());
        let mut config = FetcherConfig::default();
        config.max_recursions = 1;
        let ctx = ctx_for(&hooks, &browser, &image_cache, &style_cache, config);
        let dir = test_dir("no-content");
        let mut state = FetchState::new(&dir);

        start_fetch(&ctx, &mut state, &site.url("/start"));

        assert_eq!(state.failed_links.len(), 1);
        assert!(state.failed_links[0].1.contains("No content"), "{:?}", state.failed_links);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_soup_rewrites_absolute_local_paths_relative_to_the_target() {
        let dir = test_dir("save-soup-relpath");
        let images_dir = dir.join("images");
        std::fs::create_dir_all(&images_dir).unwrap();
        let img_path = images_dir.join("img1.png");
        std::fs::write(&img_path, b"fake png bytes").unwrap();

        let mut dom = Dom::parse(&format!(r#"<html><head></head><body><img src="{}"></body></html>"#, img_path.display()));
        let target = dir.join("index.xhtml");
        save_soup(&mut dom, &target).unwrap();

        let html = std::fs::read_to_string(&target).unwrap();
        assert!(html.contains("images/img1.png"), "{html}");
        assert!(!html.contains(&img_path.display().to_string()), "the absolute path must not survive: {html}");

        std::fs::remove_dir_all(&dir).ok();
    }

    fn image_bytes() -> Vec<u8> {
        let img = image::RgbImage::from_pixel(3, 3, image::Rgb([10, 20, 30]));
        let mut buf = Vec::new();
        image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png).unwrap();
        buf
    }
}
