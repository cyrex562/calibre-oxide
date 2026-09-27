//! `web2disk` — the console front end to the recursive fetcher (#813).
//!
//! `web::fetch::recursive::start_fetch` has been merged and tested since
//! #630-#633; it was reachable only through the news-recipe machinery, so
//! there was no way to mirror an arbitrary page from a shell. This is the
//! wrapper.
//!
//! The fetcher is built around news recipes, which is why it takes a
//! hooks object. `web2disk` is not a recipe, so it supplies an empty
//! implementation: every hook has a default, and the defaults are exactly
//! "do nothing special", which is the correct behaviour for mirroring a
//! page nobody wrote a recipe for.

use anyhow::{bail, Result};
use calibre_ebooks::scraper::Browser;
use calibre_ebooks::web::fetch::recursive::{start_fetch, FetchState, FetcherConfig, FetcherContext};
use calibre_ebooks::web::feeds::postprocess::NewsRecipePostprocessHooks;
use calibre_ebooks::web::feeds::recipe::RecipeConfig;
use clap::Parser;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(name = "web2disk")]
#[command(about = "Mirror a web page and the pages it links to, rewriting links to work offline", long_about = None)]
struct Args {
    /// The URL to start from.
    #[arg(required = true)]
    url: String,

    /// Where to write the mirror. Created if it does not exist.
    #[arg(short = 'd', long, default_value = ".")]
    base_dir: PathBuf,

    /// How many levels of links to follow. 0 fetches only the start page.
    #[arg(short = 'r', long, default_value_t = 1)]
    max_recursions: u32,

    /// Stop after this many files.
    #[arg(short = 'n', long, default_value_t = 1000)]
    max_files: usize,

    /// Seconds to wait for each request.
    #[arg(short, long, default_value_t = 10.0)]
    timeout: f64,

    /// Only follow links matching this regex. Repeatable.
    #[arg(long = "match-regexp")]
    match_regexps: Vec<String>,

    /// Skip links matching this regex. Repeatable, and applied after
    /// --match-regexp.
    #[arg(long = "filter-regexp")]
    filter_regexps: Vec<String>,

    /// Do not download linked stylesheets.
    #[arg(long)]
    dont_download_stylesheets: bool,

    /// Decode pages with this named encoding instead of detecting it.
    #[arg(long)]
    encoding: Option<String>,
}

/// The fetcher's hooks, doing nothing.
///
/// A news recipe uses these to clean up a specific site's markup. There
/// is no site-specific knowledge to apply when mirroring an arbitrary
/// URL, so every hook keeps its default.
struct PlainFetch(RecipeConfig);

impl calibre_ebooks::web::feeds::recipe::NewsRecipeHooks for PlainFetch {
    fn config(&self) -> &RecipeConfig {
        &self.0
    }
}
impl NewsRecipePostprocessHooks for PlainFetch {}

fn compile(patterns: &[String], what: &str) -> Result<Vec<regex::Regex>> {
    patterns
        .iter()
        .map(|p| regex::Regex::new(p).map_err(|e| anyhow::anyhow!("bad --{what} {p:?}: {e}")))
        .collect()
}

fn main() -> Result<()> {
    let args = Args::parse();

    if !args.url.starts_with("http://") && !args.url.starts_with("https://") {
        bail!("{} is not an http(s) URL", args.url);
    }
    std::fs::create_dir_all(&args.base_dir)?;

    let config = FetcherConfig {
        timeout: Duration::from_secs_f64(args.timeout),
        max_recursions: args.max_recursions,
        max_files: args.max_files,
        match_regexps: compile(&args.match_regexps, "match-regexp")?,
        filter_regexps: compile(&args.filter_regexps, "filter-regexp")?,
        download_stylesheets: !args.dont_download_stylesheets,
        encoding: args.encoding.clone(),
        ..Default::default()
    };

    let hooks = PlainFetch(RecipeConfig::default());
    let browser = Browser::new("", &[], true);
    let image_cache = Mutex::new(HashMap::new());
    let stylesheet_cache = Mutex::new(HashMap::new());
    let ctx = FetcherContext::new(&hooks, &browser, &image_cache, &stylesheet_cache, config, None);
    let mut state = FetchState::new(&args.base_dir);

    match start_fetch(&ctx, &mut state, &args.url) {
        Some(saved) => {
            println!("{saved}");
            // Reported because a mirror that silently dropped half the
            // pages looks identical to one that worked.
            if !state.failed_links.is_empty() {
                eprintln!("\n{} link(s) could not be fetched:", state.failed_links.len());
                for (url, why) in &state.failed_links {
                    eprintln!("  {url}: {why}");
                }
            }
            Ok(())
        }
        None => bail!("could not fetch {}", args.url),
    }
}
