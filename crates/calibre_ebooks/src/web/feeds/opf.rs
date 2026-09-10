//! Port of `BasicNewsRecipe.create_opf` (`old_src/src/calibre/web/feeds/news.py`,
//! issue #622, split from #81): assembling a downloaded periodical's
//! `index.opf`/`index.ncx` from its feeds/articles.
//!
//! Reuses the already-real, general-purpose
//! [`crate::opf_writer::{write_opf, write_ncx, auto_manifest, scan_directory_manifest}`]
//! and [`crate::metadata::toc::{TOC, TOCNode}`] (extended here with
//! `play_order`/`author`/`description`/`toc_thumbnail` and a real
//! `add_item`, matching real Python's own `calibre.ebooks.metadata.toc.TOC`
//! shape exactly -- these fields render as real NCX `<calibre:meta>`
//! extension elements, see `opf_writer::write_ncx`'s own doc).
//!
//! [`build_periodical_toc`] is the pure, directly-testable half of
//! `create_opf` (given a set of `Feed`/`Article` fixtures -- real or
//! synthetic -- it returns the TOC tree and spine order with no
//! filesystem access), matching the issue's own suggested approach of
//! developing this against synthetic "already downloaded" fixtures,
//! decoupled from #623's real fetch orchestration. [`create_opf`]
//! itself does need the filesystem, to reuse
//! [`crate::opf_writer::scan_directory_manifest`] for a real manifest
//! of whatever files a real fetch pass already wrote to `output_dir`.
//!
//! # Scope
//!
//! **Disclosed narrowing, deferred to #623**: real `create_opf`'s
//! `feed_index` closure also rewrites each already-downloaded
//! article's saved HTML file in place, appending a *bottom* navbar
//! (`self.navbar.generate(True, ...)`, distinct from the *top* navbar
//! `_postprocess_html`/#620 already inserts during the initial
//! download pass) and tracking `self.article_url_map` for
//! `internal_postprocess_book`'s internal-link resolution (a *third*,
//! separate real hook -- an OEB-conversion-time `postprocess_book`
//! plugin hook, not mentioned in this issue's own body, and out of
//! scope here). Neither has a real consumer yet in this port
//! (`internal_postprocess_book` isn't ported, and re-injecting into
//! already-saved files needs the live output paths #623's real fetch
//! orchestration produces) -- the same "needs live state this pure
//! transform doesn't have" reasoning #620 already used for
//! `populate_article_metadata`. `generate_navbar(bottom: true, ...)`
//! is already real and ready in `templates.rs` for #623 to use.
//!
//! **Disclosed narrowing**: real `create_opf`'s `article_titles` list
//! (feeding into `mi.comments`) excludes articles recorded in
//! `self.aborted_articles`/`self.failed_downloads` -- state this port
//! has no equivalent of yet (#623 territory, real fetch-failure
//! tracking). [`create_opf`] includes every article title across
//! `feeds` instead; harmless in practice (cosmetic description text
//! only), and callers passing only successfully-downloaded articles
//! (the common case) see no difference at all.
//!
//! **Disclosed narrowing**: `periodical_date_in_title` is a real
//! `OutputProfile` field (`self.output_profile.periodical_date_in_title`)
//! this port has no `OutputProfile` selection system for yet -- any
//! date suffix on `title` is the caller's own responsibility (already
//! this cluster's established convention: `templates.rs`'s
//! `generate_index`/`generate_touchscreen_index` also take a
//! pre-formatted `date_str` rather than a `timefmt` string, issue #618).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::metadata::toc::{TOCNode, TOC};
use crate::metadata::MetaInformation;
use crate::opf_writer::{self, auto_manifest, scan_directory_manifest, GuideRef, ManifestItem};

use super::recipe::description_limiter;
use super::Feed;

/// Either the root [`TOC`] or an already-added [`TOCNode`] -- the two
/// real receivers of `TOC.add_item` in `create_opf`'s own
/// `feed_index`/top-level loop (a single-feed periodical adds
/// articles straight onto the root; a multi-feed one adds them under
/// each feed's own node).
enum TocParent<'a> {
    Root(&'a mut TOC),
    Node(&'a mut TOCNode),
}

impl TocParent<'_> {
    fn add_item(&mut self, href: String, title: String, play_order: u32, author: Option<String>, description: Option<String>, toc_thumbnail: Option<String>) -> &mut TOCNode {
        match self {
            TocParent::Root(toc) => toc.add_item(href, title, play_order, author, description, toc_thumbnail),
            TocParent::Node(node) => node.add_item(href, title, play_order, author, description, toc_thumbnail),
        }
    }
}

fn feed_index(feed_idx: usize, feed: &Feed, parent: &mut TocParent, entries: &mut Vec<String>, play_order_counter: &mut u32, summary_length: usize) {
    for (j, article) in feed.articles.iter().enumerate() {
        if !article.downloaded {
            continue;
        }
        let adir = format!("feed_{feed_idx}/article_{j}/");
        let arelpath = format!("{adir}index.html");
        entries.push(arelpath.clone());

        *play_order_counter += 1;
        let play_order = *play_order_counter;

        let desc = if article.text_summary.is_empty() { None } else { Some(description_limiter(&article.text_summary, summary_length)) };
        let title = if article.title.is_empty() { "Untitled article".to_string() } else { article.title.clone() };

        let article_node = parent.add_item(arelpath.clone(), title, play_order, article.author.clone(), desc, article.toc_thumbnail.clone());

        for entry in &article.internal_toc_entries {
            let Some(anchor) = &entry.anchor else { continue };
            *play_order_counter += 1;
            let sub_play_order = *play_order_counter;
            let entry_title = entry.title.clone().unwrap_or_else(|| "Unknown section".to_string());
            article_node.add_item(format!("{arelpath}#{anchor}"), entry_title, sub_play_order, None, None, None);
        }

        for sub_page in &article.sub_pages {
            entries.push(sub_page.clone());
        }
    }
}

/// Port of the TOC-building/spine-ordering half of `create_opf`. Pure
/// (no filesystem access) -- directly testable against synthetic
/// `Feed`/`Article` fixtures. Returns the built [`TOC`] and the spine
/// entries (as `href`s relative to the periodical's output directory,
/// e.g. `"feed_0/article_2/index.html"`) in real Python's own order.
///
/// Errors if `feeds` is empty, matching real `create_opf`'s own
/// `raise Exception('All feeds are empty, aborting.')`.
pub fn build_periodical_toc(feeds: &[Feed], summary_length: usize) -> anyhow::Result<(TOC, Vec<String>)> {
    if feeds.is_empty() {
        anyhow::bail!("All feeds are empty, aborting.");
    }

    let mut toc = TOC::new();
    let mut entries = vec!["index.html".to_string()];
    let mut play_order_counter: u32 = 0;

    if feeds.len() > 1 {
        for (i, feed) in feeds.iter().enumerate() {
            entries.push(format!("feed_{i}/index.html"));
            play_order_counter += 1;
            let play_order = play_order_counter;
            let desc = if feed.description.is_empty() { None } else { Some(feed.description.clone()) };
            // Real Python's `getattr(f, 'author', None)` is always
            // `None` here -- `Feed` never sets a `self.author`
            // attribute anywhere (confirmed by reading `__init__.py`
            // directly), the same kind of dead branch #618 found for
            // `feed.image`.
            let feed_node = toc.add_item(format!("feed_{i}/index.html"), feed.title.clone(), play_order, None, desc, None);
            feed_index(i, feed, &mut TocParent::Node(feed_node), &mut entries, &mut play_order_counter, summary_length);
        }
    } else {
        entries.push("feed_0/index.html".to_string());
        feed_index(0, &feeds[0], &mut TocParent::Root(&mut toc), &mut entries, &mut play_order_counter, summary_length);
    }

    Ok((toc, entries))
}

/// Real inputs `create_opf` needs beyond `feeds` itself.
pub struct CreateOpfOptions<'a> {
    /// The periodical's title, already formatted the way the caller
    /// wants it to appear (see this module's own doc for why any
    /// `periodical_date_in_title` suffix is the caller's job).
    pub title: &'a str,
    pub description: &'a str,
    /// Raw language name/code; canonicalized internally via
    /// `calibre_utils::localization::canonicalize_lang`.
    pub language: &'a str,
    /// Port of `self.publication_type`; rendered as
    /// `periodical:{publication_type}:{title}`, matching real Python.
    pub publication_type: Option<&'a str>,
    pub pubdate: DateTime<Utc>,
    pub summary_length: usize,
    /// An already-downloaded cover image's path within `output_dir`,
    /// if one exists (relative to `output_dir`, e.g. `"cover.jpg"`).
    /// `None` synthesizes one via [`super::cover::default_cover`],
    /// matching real `create_opf`'s own `default_cover` fallback.
    pub cover_relpath: Option<&'a str>,
    /// An already-prepared masthead image's path within `output_dir`,
    /// if one exists (e.g. `"mastheadImage.jpg"`).
    pub masthead_relpath: Option<&'a str>,
}

/// Port of `BasicNewsRecipe.create_opf`. Builds the TOC (via
/// [`build_periodical_toc`]), assembles `MetaInformation` and a
/// manifest from whatever files already exist under `output_dir`
/// (via [`crate::opf_writer::scan_directory_manifest`]), and writes
/// `index.opf`/`index.ncx` there.
pub fn create_opf(output_dir: &Path, feeds: &[Feed], opts: &CreateOpfOptions, db: &Arc<fontdb::Database>) -> anyhow::Result<()> {
    let (toc, entries) = build_periodical_toc(feeds, opts.summary_length)?;

    let mut article_titles = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for feed in feeds {
        for article in &feed.articles {
            if !article.title.is_empty() && seen.insert(article.title.clone()) {
                article_titles.push(article.title.clone());
            }
        }
    }
    let comments = format!("Articles in this issue:\n\n{}\n\n{}", article_titles.join("\n\n"), opts.description);

    let app_name = calibre_utils::constants::APP_NAME;
    let mut mi = MetaInformation::new(opts.title, vec![app_name.to_string()]);
    mi.publisher = Some(app_name.to_string());
    mi.author_sort = Some(app_name.to_string());
    mi.publication_type = opts.publication_type.map(|t| format!("periodical:{t}:{}", opts.title));
    mi.timestamp = Some(Utc::now());
    mi.comments = Some(comments);
    mi.languages = calibre_utils::localization::canonicalize_lang(opts.language).into_iter().collect();
    mi.pubdate = Some(opts.pubdate);

    let mut guide = Vec::new();
    if let Some(masthead_relpath) = opts.masthead_relpath {
        if output_dir.join(masthead_relpath).is_file() {
            guide.push(GuideRef { type_: "masthead".to_string(), title: "Masthead Image".to_string(), href: masthead_relpath.to_string() });
        }
    }

    let cover_relpath = match opts.cover_relpath.filter(|p| output_dir.join(p).is_file()) {
        Some(p) => Some(p.to_string()),
        None => match super::cover::default_cover(opts.title, &opts.pubdate.format("%Y-%m-%d").to_string(), db) {
            Some(data) => {
                std::fs::write(output_dir.join("cover.jpg"), data)?;
                Some("cover.jpg".to_string())
            }
            None => None,
        },
    };

    let scanned = scan_directory_manifest(output_dir, &["index.opf", "index.ncx"]);
    let mut manifest: Vec<ManifestItem> = auto_manifest(&scanned);
    // Port of `for mani in opf.manifest: if mani.path.endswith('mastheadImage.jpg'): mani.id = 'masthead-image'`.
    for item in &mut manifest {
        if item.href.ends_with("mastheadImage.jpg") {
            item.id = "masthead-image".to_string();
        }
    }
    manifest.push(ManifestItem::new("ncx", "index.ncx", "application/x-dtbncx+xml"));

    let href_to_id: HashMap<&str, &str> = manifest.iter().map(|m| (m.href.as_str(), m.id.as_str())).collect();
    let spine_idrefs: Vec<String> = entries.iter().filter_map(|href| href_to_id.get(href.as_str()).map(|id| id.to_string())).collect();

    let opf_xml = opf_writer::write_opf(&mi, &manifest, &spine_idrefs, &guide, Some("ncx"), Some("index.ncx"), cover_relpath.as_deref(), None, None);
    let ncx_xml = opf_writer::write_ncx(&toc, "urn:uuid:periodical", &mi.title);

    std::fs::write(output_dir.join("index.opf"), opf_xml)?;
    std::fs::write(output_dir.join("index.ncx"), ncx_xml)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::feeds::Article;
    use crate::web::feeds::InternalTocEntry;
    use chrono::TimeZone;
    use std::sync::OnceLock;

    fn test_db() -> &'static Arc<fontdb::Database> {
        static DB: OnceLock<Arc<fontdb::Database>> = OnceLock::new();
        DB.get_or_init(|| {
            let mut db = fontdb::Database::new();
            db.load_system_fonts();
            Arc::new(db)
        })
    }

    fn downloaded_article(title: &str) -> Article {
        let mut a = Article::new("id", Some(title), Some(format!("https://example.com/{title}")), Some("Jane Doe".to_string()), Some("Summary text.".to_string()), None, None);
        a.downloaded = true;
        a
    }

    fn feed_with(title: &str, articles: Vec<Article>) -> Feed {
        let mut feed = Feed::populate_from_preparsed_feed(Some(title), &[], 36500.0, 100);
        feed.description = "A description".to_string();
        feed.articles = articles;
        feed
    }

    #[test]
    fn build_periodical_toc_errors_on_no_feeds() {
        assert!(build_periodical_toc(&[], 500).is_err());
    }

    #[test]
    fn build_periodical_toc_single_feed_adds_articles_at_the_root() {
        let feed = feed_with("Feed One", vec![downloaded_article("Article One"), downloaded_article("Article Two")]);
        let (toc, entries) = build_periodical_toc(&[feed], 500).unwrap();

        assert_eq!(toc.nodes.len(), 2, "single-feed periodicals add articles directly at the TOC root");
        assert_eq!(toc.nodes[0].title, "Article One");
        assert_eq!(toc.nodes[0].src, "feed_0/article_0/index.html");
        assert_eq!(toc.nodes[0].play_order, Some(1));
        assert_eq!(toc.nodes[1].play_order, Some(2));
        assert_eq!(toc.nodes[0].author.as_deref(), Some("Jane Doe"));

        assert_eq!(entries, vec!["index.html", "feed_0/index.html", "feed_0/article_0/index.html", "feed_0/article_1/index.html"]);
    }

    #[test]
    fn build_periodical_toc_multi_feed_nests_articles_under_their_feed() {
        let feed_a = feed_with("Feed A", vec![downloaded_article("A1")]);
        let feed_b = feed_with("Feed B", vec![downloaded_article("B1"), downloaded_article("B2")]);
        let (toc, entries) = build_periodical_toc(&[feed_a, feed_b], 500).unwrap();

        assert_eq!(toc.nodes.len(), 2);
        assert_eq!(toc.nodes[0].title, "Feed A");
        assert_eq!(toc.nodes[0].src, "feed_0/index.html");
        assert_eq!(toc.nodes[0].children.len(), 1);
        assert_eq!(toc.nodes[0].children[0].title, "A1");
        assert_eq!(toc.nodes[1].children.len(), 2);
        assert_eq!(toc.nodes[1].children[1].title, "B2");

        // Play order is a single, global, depth-first counter spanning
        // both feed nodes and their articles.
        assert_eq!(toc.nodes[0].play_order, Some(1));
        assert_eq!(toc.nodes[0].children[0].play_order, Some(2));
        assert_eq!(toc.nodes[1].play_order, Some(3));
        assert_eq!(toc.nodes[1].children[0].play_order, Some(4));
        assert_eq!(toc.nodes[1].children[1].play_order, Some(5));

        assert_eq!(
            entries,
            vec!["index.html", "feed_0/index.html", "feed_0/article_0/index.html", "feed_1/index.html", "feed_1/article_0/index.html", "feed_1/article_1/index.html"]
        );
    }

    #[test]
    fn build_periodical_toc_skips_articles_that_were_not_downloaded() {
        let mut not_downloaded = downloaded_article("Skipped");
        not_downloaded.downloaded = false;
        let feed = feed_with("Feed", vec![downloaded_article("Kept"), not_downloaded]);
        let (toc, entries) = build_periodical_toc(&[feed], 500).unwrap();

        assert_eq!(toc.nodes.len(), 1);
        assert_eq!(toc.nodes[0].title, "Kept");
        assert!(!entries.iter().any(|e| e.contains("article_1")));
    }

    #[test]
    fn build_periodical_toc_adds_internal_toc_entries_under_their_article() {
        let mut article = downloaded_article("Long Article");
        article.internal_toc_entries = vec![
            InternalTocEntry { anchor: Some("s1".to_string()), title: Some("Section One".to_string()) },
            InternalTocEntry { anchor: None, title: Some("Skipped (no anchor)".to_string()) },
            InternalTocEntry { anchor: Some("s2".to_string()), title: None },
        ];
        let feed = feed_with("Feed", vec![article]);
        let (toc, _entries) = build_periodical_toc(&[feed], 500).unwrap();

        assert_eq!(toc.nodes[0].children.len(), 2, "only anchored entries become TOC children");
        assert_eq!(toc.nodes[0].children[0].src, "feed_0/article_0/index.html#s1");
        assert_eq!(toc.nodes[0].children[0].title, "Section One");
        assert_eq!(toc.nodes[0].children[1].title, "Unknown section");
        // Play order continues from the article's own value.
        assert_eq!(toc.nodes[0].play_order, Some(1));
        assert_eq!(toc.nodes[0].children[0].play_order, Some(2));
        assert_eq!(toc.nodes[0].children[1].play_order, Some(3));
    }

    #[test]
    fn build_periodical_toc_includes_sub_pages_in_spine_order() {
        let mut article = downloaded_article("Split Article");
        article.sub_pages = vec!["feed_0/article_0/split_001.html".to_string(), "feed_0/article_0/split_002.html".to_string()];
        let feed = feed_with("Feed", vec![article]);
        let (_toc, entries) = build_periodical_toc(&[feed], 500).unwrap();

        assert_eq!(
            entries,
            vec!["index.html", "feed_0/index.html", "feed_0/article_0/index.html", "feed_0/article_0/split_001.html", "feed_0/article_0/split_002.html"]
        );
    }

    #[test]
    fn build_periodical_toc_uses_a_fallback_title_for_untitled_articles() {
        let mut article = downloaded_article("Unknown");
        article.title = String::new();
        let feed = feed_with("Feed", vec![article]);
        let (toc, _entries) = build_periodical_toc(&[feed], 500).unwrap();
        assert_eq!(toc.nodes[0].title, "Untitled article");
    }

    #[test]
    fn create_opf_writes_a_real_opf_and_ncx_from_downloaded_fixtures() {
        let dir = std::env::temp_dir().join(format!("calibre-oxide-test-create-opf-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("feed_0/article_0")).unwrap();
        std::fs::write(dir.join("feed_0/article_0/index.html"), "<html><body>Article</body></html>").unwrap();
        std::fs::write(dir.join("feed_0/index.html"), "<html><body>Feed</body></html>").unwrap();
        std::fs::write(dir.join("index.html"), "<html><body>Index</body></html>").unwrap();

        let feed = feed_with("Feed", vec![downloaded_article("Article")]);
        let opts = CreateOpfOptions {
            title: "My Weekly",
            description: "A test periodical",
            language: "en",
            publication_type: Some("news"),
            pubdate: Utc.with_ymd_and_hms(2026, 9, 10, 0, 0, 0).unwrap(),
            summary_length: 500,
            cover_relpath: None,
            masthead_relpath: None,
        };

        create_opf(&dir, &[feed], &opts, test_db()).unwrap();

        let opf = std::fs::read_to_string(dir.join("index.opf")).unwrap();
        assert!(opf.contains("<dc:title>My Weekly</dc:title>"), "{opf}");
        assert!(opf.contains("calibre:publication_type"), "{opf}");
        assert!(opf.contains("periodical:news:My Weekly"), "{opf}");
        assert!(opf.contains("<dc:language>eng</dc:language>"), "{opf}");
        assert!(opf.contains("meta name=\"cover\""), "a synthesized default cover should be referenced: {opf}");
        assert!(opf.contains("href=\"index.ncx\""), "{opf}");
        assert!(opf.contains("itemref idref="), "{opf}");

        let ncx = std::fs::read_to_string(dir.join("index.ncx")).unwrap();
        assert!(ncx.contains("Article"), "{ncx}");
        assert!(ncx.contains("playOrder=\"1\""), "{ncx}");
        assert!(dir.join("cover.jpg").is_file(), "default_cover should have written a real cover.jpg");

        std::fs::remove_dir_all(&dir).ok();
    }
}
