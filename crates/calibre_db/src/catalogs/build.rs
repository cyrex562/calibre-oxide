//! Port of `epub_mobi_builder.py`'s top-level `CatalogBuilder` orchestration
//! (the parts of `__init__` that aren't just field initialization, plus
//! `build_sources`) -- cluster F, the final piece of the
//! `epub_mobi_builder.py` port (issue #57). [`build_catalog`] threads
//! together every already-ported cluster (A: data prep, B: sort/key
//! helpers, C: HTML section generators, D: NCX navigation, E: OPF +
//! thumbnails) in the same order as upstream's own `build_sources`.
//!
//! # Disclosed simplifications
//!
//! - **`fetch_bookmarks`/date-read sections skipped.** `bookmarked_books`
//!   is permanently empty in this port (`fetch_bookmarks` was never
//!   ported -- device-specific, and upstream's own docstring already
//!   calls it "Turned off ... as of 0.8.70"), so
//!   `generate_html_by_date_read`/`generate_ncx_by_date_read` (also never
//!   ported) are unreachable and skipped here too, matching upstream's
//!   own `if self.generate_recently_read:` guard, which is always false
//!   under the same condition.
//! - **`generate_masthead_image` not invoked.** Not ported (see
//!   `thumbnails.rs`'s doc) -- [`epub_mobi_builder::copy_catalog_resources`]
//!   copies the static `mastheadImage.gif` resource as-is instead of
//!   regenerating it with the catalog's own title text baked in.
//! - **`epub_mobi.py`'s `EPUB_MOBI.run()` itself is out of scope.** That's
//!   CLI-option resolution plus a final `Plumber` ebook-conversion
//!   handoff (turning this catalog's HTML/OPF/NCX tree into an actual
//!   `.epub`/`.mobi`/`.azw3` file) -- a wholly different subsystem
//!   (`ebooks/conversion`) this crate has no port of yet. [`build_catalog`]
//!   stops exactly where upstream's own `CatalogBuilder.build_sources`
//!   stops: a directory of HTML/OPF/NCX content ready for that
//!   conversion step.
//! - **`only_genres_selected`** replaces upstream's `self.opts.section_list
//!   == ['Genres']` (this port has no `section_list` option type) --
//!   callers pass whether genres were the *only* section requested,
//!   which is what that comparison actually decides.

use std::path::PathBuf;

use serde_json::Value;

use crate::cache::Cache;

use super::epub_mobi_builder::{
    self, FetchBooksByAuthorResult, GenrePage, PopulateTitleOptions, PrefixRule,
};
use super::ncx::{self, NcxBuilder};
use super::opf::{self, OpfOptions};
use super::output_profiles;
use super::thumbnails;
use super::{CatalogError, Result};

const CONTENT_DIR: &str = "content";

/// Everything [`build_catalog`] needs that upstream reads off
/// `self.opts`/`self` -- see this module's doc for what's simplified or
/// renamed relative to upstream's field names.
#[derive(Debug, Clone)]
pub struct CatalogBuildOptions {
    pub ids: Option<Vec<i32>>,
    pub exclusion_rules: Vec<(String, String, String)>,
    pub prefix_rules: Vec<(String, String, String, String)>,
    pub populate_title: PopulateTitleOptions,
    pub fmt: String,
    pub generate_for_kindle_mobi: bool,
    pub generate_authors: bool,
    pub generate_titles: bool,
    pub generate_series: bool,
    pub generate_genres: bool,
    pub generate_recently_added: bool,
    pub generate_descriptions: bool,
    pub cross_reference_authors: bool,
    pub sort_descriptions_by_author: bool,
    /// Upstream's `self.opts.section_list == ['Genres']` -- see this
    /// module's doc.
    pub only_genres_selected: bool,
    pub date_ranges_days: Vec<i64>,
    pub now: chrono::DateTime<chrono::Utc>,
    pub output_profile: String,
    pub thumb_width_inches: f64,
    pub catalog_title: String,
    pub creator: String,
    pub lang: String,
    pub basename: String,
    pub stylesheet: String,
    pub catalog_path: PathBuf,
    /// Upstream's `cache_dir()/catalog` -- holds the cross-run
    /// `thumbs.zip` cache (`confirm_thumbs_archive`'s own `cache_dir`
    /// parameter).
    pub cache_dir: PathBuf,
    pub resources_dir: PathBuf,
    pub default_cover_path: Option<PathBuf>,
}

/// Everything [`build_catalog`] produces: HTML/OPF/NCX content plus
/// non-fatal warnings, all as in-memory strings (matching this whole
/// port's "pure function, caller does I/O" convention) except for the
/// thumbnail images themselves, which [`super::thumbnails::generate_thumbnails`]
/// already writes to `catalog_path/images` directly (a genuinely I/O-bound
/// step, same exception as `create_catalog_directory_structure`/
/// `copy_catalog_resources`).
#[derive(Debug, Clone, Default)]
pub struct CatalogBuildOutput {
    /// `(path relative to catalog_path, content)` -- HTML section pages,
    /// genre pages, and per-book description pages.
    pub html_files: Vec<(String, String)>,
    pub opf: String,
    pub ncx: String,
    /// Thumbnail filenames already written under `catalog_path/images`
    /// by [`super::thumbnails::generate_thumbnails`].
    pub thumbs: Vec<String>,
    pub warnings: Vec<String>,
}

/// Port of `CatalogBuilder.build_sources`, plus the handful of
/// `__init__` steps `build_sources` itself depends on
/// (`filter_genre_tags`, `fetch_books_to_catalog`,
/// `calculate_thumbnail_dimensions`, `confirm_thumbs_archive`,
/// directory/resource setup) that upstream computes once at
/// construction time rather than inside `build_sources` proper.
pub fn build_catalog(db: &Cache, opts: &CatalogBuildOptions) -> Result<CatalogBuildOutput> {
    let prefix_rules: Vec<PrefixRule> = epub_mobi_builder::get_prefix_rules(&opts.prefix_rules);

    let genre_tags_dict = if opts.generate_genres {
        let excluded_tags = epub_mobi_builder::get_excluded_tags(&opts.exclusion_rules);
        let genre_html_path = opts.catalog_path.join(CONTENT_DIR).join("Genre_.html");
        let max_len = 245usize.saturating_sub(genre_html_path.to_string_lossy().len());
        epub_mobi_builder::filter_genre_tags(db, max_len, &excluded_tags, &opts.populate_title.exclude_genre)?
    } else {
        indexmap::IndexMap::new()
    };

    let books_to_catalog = epub_mobi_builder::fetch_books_to_catalog(
        db,
        opts.ids.as_deref(),
        &opts.exclusion_rules,
        &prefix_rules,
        &opts.populate_title,
    )?;

    let profile = output_profiles::get_output_profile(&opts.output_profile)
        .or_else(|| output_profiles::get_output_profile("default"))
        .expect("the \"default\" output profile always exists");
    let rating_full_char = profile.ratings_char;
    let rating_empty_char = profile.empty_ratings_char;
    let (thumb_width, thumb_height) =
        output_profiles::calculate_thumbnail_dimensions(&opts.output_profile, opts.thumb_width_inches, &opts.fmt);

    let thumbs_path = opts.cache_dir.join("thumbs.zip");
    thumbnails::confirm_thumbs_archive(&opts.cache_dir, &thumbs_path, thumb_width)
        .map_err(CatalogError::Db)?;

    epub_mobi_builder::copy_catalog_resources(&opts.resources_dir, &opts.catalog_path, opts.generate_for_kindle_mobi)
        .map_err(|e| CatalogError::Db(e.into()))?;

    let books_by_title = epub_mobi_builder::fetch_books_by_title(&books_to_catalog)?;

    let FetchBooksByAuthorResult { books_by_author, books_by_description, authors, individual_authors, warnings } =
        epub_mobi_builder::fetch_books_by_author(
            &books_to_catalog,
            Some(&books_by_title),
            opts.cross_reference_authors,
            opts.generate_descriptions,
            opts.sort_descriptions_by_author,
            &opts.fmt,
        )?;
    let books_by_description = books_by_description.unwrap_or_default();

    let mut html_files: Vec<(String, String)> = Vec::new();
    let mut thumbs: Vec<String> = Vec::new();

    if opts.generate_descriptions {
        thumbs = thumbnails::generate_thumbnails(
            &books_by_title,
            &opts.catalog_path,
            &thumbs_path,
            thumb_width.round() as u32,
            thumb_height.round() as u32,
            opts.default_cover_path.as_deref(),
        )
        .map_err(CatalogError::Db)?;

        for (book_id, html) in epub_mobi_builder::generate_html_descriptions(
            &books_by_title,
            &genre_tags_dict,
            opts.generate_genres,
            opts.generate_series,
            opts.generate_authors,
            rating_full_char,
            rating_empty_char,
        ) {
            html_files.push((format!("{CONTENT_DIR}/book_{book_id}.html"), html));
        }
    }

    let mut html_filelist_1: Vec<String> = Vec::new();
    if opts.generate_authors {
        let html = epub_mobi_builder::generate_html_by_author(
            &books_by_author,
            &opts.fmt,
            opts.generate_for_kindle_mobi,
            opts.generate_series,
            opts.generate_descriptions,
            rating_full_char,
            rating_empty_char,
        );
        let path = format!("{CONTENT_DIR}/ByAlphaAuthor.html");
        html_filelist_1.push(path.clone());
        html_files.push((path, html));
    }
    if opts.generate_titles {
        let html = epub_mobi_builder::generate_html_by_title(
            &books_by_title,
            &opts.fmt,
            opts.generate_for_kindle_mobi,
            opts.generate_descriptions,
            opts.generate_authors,
            rating_full_char,
            rating_empty_char,
        );
        let path = format!("{CONTENT_DIR}/ByAlphaTitle.html");
        html_filelist_1.push(path.clone());
        html_files.push((path, html));
    }
    if opts.generate_series {
        if let Some(html) = epub_mobi_builder::generate_html_by_series(
            db,
            &books_to_catalog,
            &prefix_rules,
            &opts.fmt,
            opts.generate_for_kindle_mobi,
            opts.generate_descriptions,
            opts.generate_authors,
            rating_full_char,
            rating_empty_char,
        )? {
            let path = format!("{CONTENT_DIR}/BySeries.html");
            html_filelist_1.push(path.clone());
            html_files.push((path, html));
        }
    }

    let genres: Vec<GenrePage> = if opts.generate_genres {
        let pages = epub_mobi_builder::generate_html_by_genres(
            &genre_tags_dict,
            &books_by_author,
            &opts.fmt,
            opts.generate_authors,
            opts.generate_series,
            opts.generate_descriptions,
            rating_full_char,
            rating_empty_char,
        );
        if opts.only_genres_selected && pages.is_empty() {
            return Err(CatalogError::EmptyCatalog);
        }
        pages
    } else {
        Vec::new()
    };
    for genre in &genres {
        html_files.push((genre.file.clone(), genre.html.clone()));
    }

    let mut html_filelist_2: Vec<String> = Vec::new();
    if opts.generate_recently_added {
        let html = epub_mobi_builder::generate_html_by_date_added(
            &books_to_catalog,
            &opts.date_ranges_days,
            opts.now,
            &opts.fmt,
            opts.generate_for_kindle_mobi,
            opts.generate_series,
            opts.generate_authors,
            opts.generate_descriptions,
            rating_full_char,
            rating_empty_char,
        );
        let path = format!("{CONTENT_DIR}/ByDateAdded.html");
        html_filelist_2.push(path.clone());
        html_files.push((path, html));
    }

    let opf_opts = OpfOptions {
        catalog_title: opts.catalog_title.clone(),
        creator: opts.creator.clone(),
        lang: opts.lang.clone(),
        generate_for_kindle_mobi: opts.generate_for_kindle_mobi,
        basename: opts.basename.clone(),
        stylesheet: opts.stylesheet.clone(),
        generate_descriptions: opts.generate_descriptions,
    };
    let opf_xml = opf::generate_opf(&opf_opts, &thumbs, &html_filelist_1, &genres, &html_filelist_2, &books_by_description);

    let first_genre_file = genres.first().map(|g| g.file.as_str());
    let first_description_book_id = books_by_description.first().and_then(|b| b.get("id").and_then(Value::as_i64));
    let mut ncx_builder = NcxBuilder::new(
        opts.generate_for_kindle_mobi,
        &opts.catalog_title,
        opts.generate_authors,
        opts.generate_titles,
        opts.generate_series,
        opts.generate_genres,
        opts.generate_recently_added,
        opts.generate_descriptions,
        first_genre_file,
        first_description_book_id,
    );

    if opts.generate_authors {
        ncx::generate_ncx_by_author(
            &mut ncx_builder,
            "Authors",
            &authors,
            individual_authors.len(),
            opts.generate_for_kindle_mobi,
            opts.populate_title.description_clip,
        );
    }
    if opts.generate_titles {
        ncx::generate_ncx_by_title(
            &mut ncx_builder,
            "Titles",
            &books_by_title,
            opts.generate_for_kindle_mobi,
            opts.populate_title.description_clip,
        );
    }
    if opts.generate_series {
        let books_by_series = epub_mobi_builder::compute_books_by_series(&books_to_catalog);
        let all_series_count = books_to_catalog
            .iter()
            .filter_map(|b| b.get("series").and_then(Value::as_str).filter(|s| !s.is_empty()))
            .collect::<std::collections::HashSet<_>>()
            .len();
        ncx::generate_ncx_by_series(
            &mut ncx_builder,
            "Series",
            &books_by_series,
            all_series_count,
            opts.generate_for_kindle_mobi,
            opts.populate_title.description_clip,
        );
    }
    if opts.generate_genres {
        ncx::generate_ncx_by_genre(
            &mut ncx_builder,
            "Genres",
            &genres,
            &genre_tags_dict,
            opts.generate_for_kindle_mobi,
            opts.populate_title.description_clip,
        );
    }
    if opts.generate_recently_added {
        ncx::generate_ncx_by_date_added(
            &mut ncx_builder,
            "Recently Added",
            &books_to_catalog,
            &opts.date_ranges_days,
            opts.now,
            opts.populate_title.description_clip,
        );
    }
    if opts.generate_descriptions {
        ncx::generate_ncx_descriptions(
            &mut ncx_builder,
            "Descriptions",
            &books_by_description,
            opts.generate_for_kindle_mobi,
            opts.populate_title.author_clip,
            opts.populate_title.description_clip,
        );
    }
    let ncx_xml = ncx_builder.write();

    Ok(CatalogBuildOutput { html_files, opf: opf_xml, ncx: ncx_xml, thumbs, warnings })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn open_test_cache() -> (tempfile::TempDir, Cache) {
        let dir = tempdir().unwrap();
        let cache = Cache::new(dir.path()).expect("Cache::new should succeed");
        (dir, cache)
    }

    fn add_test_book(dir: &std::path::Path, cache: &Cache, title: &str, author: &str) -> i32 {
        let source = dir.join(format!("{title}.epub"));
        std::fs::write(&source, b"x").unwrap();
        let mut meta = calibre_ebooks::metadata::MetaInformation::default();
        meta.title = title.to_string();
        meta.authors = vec![author.to_string()];
        cache.add_book(&source, &meta).unwrap()
    }

    /// The reference `catalog` resource tree `copy_catalog_resources`
    /// needs (`DefaultCover.jpg`/`stylesheet.css`/`mastheadImage.gif`),
    /// or `None` if this checkout doesn't have it -- same
    /// skip-gracefully convention as `epub_mobi_builder.rs`'s own
    /// `copy_catalog_resources` tests.
    fn test_resources_dir() -> Option<PathBuf> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../old_src/resources/catalog");
        dir.join("DefaultCover.jpg").exists().then_some(dir)
    }

    fn base_opts(catalog_path: PathBuf, cache_dir: PathBuf, resources_dir: PathBuf) -> CatalogBuildOptions {
        CatalogBuildOptions {
            ids: None,
            exclusion_rules: vec![],
            prefix_rules: vec![],
            populate_title: PopulateTitleOptions {
                exclude_genre: r"\[.+\]|^\+$".to_string(),
                genre_source_field: "Tags".to_string(),
                description_clip: 380,
                author_clip: 100,
                ..Default::default()
            },
            fmt: "epub".to_string(),
            generate_for_kindle_mobi: false,
            generate_authors: false,
            generate_titles: false,
            generate_series: false,
            generate_genres: false,
            generate_recently_added: false,
            generate_descriptions: false,
            cross_reference_authors: false,
            sort_descriptions_by_author: false,
            only_genres_selected: false,
            date_ranges_days: vec![30, 90, 180],
            now: chrono::Utc::now(),
            output_profile: "default".to_string(),
            thumb_width_inches: 1.0,
            catalog_title: "My Catalog".to_string(),
            creator: "calibre-oxide".to_string(),
            lang: "en".to_string(),
            basename: "catalog".to_string(),
            stylesheet: "stylesheet.css".to_string(),
            catalog_path,
            cache_dir,
            resources_dir,
            default_cover_path: None,
        }
    }

    #[test]
    fn build_catalog_generates_author_title_and_series_sections() {
        let Some(resources_dir) = test_resources_dir() else { return };
        let (dir, cache) = open_test_cache();
        let id1 = add_test_book(dir.path(), &cache, "The First Book", "Jane Doe");
        cache.set_field(id1, "series", "A Series").unwrap();
        add_test_book(dir.path(), &cache, "Another Book", "John Smith");

        let catalog_path = dir.path().join("catalog");
        let cache_dir = dir.path().join("cache");
        let mut opts = base_opts(catalog_path.clone(), cache_dir, resources_dir);
        opts.generate_authors = true;
        opts.generate_titles = true;
        opts.generate_series = true;

        let out = build_catalog(&cache, &opts).expect("build_catalog should succeed");

        let paths: Vec<&str> = out.html_files.iter().map(|(p, _)| p.as_str()).collect();
        assert!(paths.contains(&"content/ByAlphaAuthor.html"));
        assert!(paths.contains(&"content/ByAlphaTitle.html"));
        assert!(paths.contains(&"content/BySeries.html"));

        let by_author = &out.html_files.iter().find(|(p, _)| p == "content/ByAlphaAuthor.html").unwrap().1;
        assert!(by_author.contains("Jane Doe") && by_author.contains("John Smith"));

        assert!(out.opf.contains("My Catalog"));
        assert!(out.ncx.contains("<ncx"));
        assert!(out.thumbs.is_empty());
    }

    #[test]
    fn build_catalog_skips_unrequested_sections() {
        let Some(resources_dir) = test_resources_dir() else { return };
        let (dir, cache) = open_test_cache();
        add_test_book(dir.path(), &cache, "Solo Book", "Solo Author");

        let catalog_path = dir.path().join("catalog");
        let cache_dir = dir.path().join("cache");
        let opts = base_opts(catalog_path, cache_dir, resources_dir);

        let out = build_catalog(&cache, &opts).unwrap();
        assert!(out.html_files.is_empty());
        assert!(out.opf.contains("<package"));
        assert!(out.ncx.contains("<ncx"));
    }

    #[test]
    fn build_catalog_bails_with_empty_catalog_when_only_genres_selected_and_none_match() {
        let Some(resources_dir) = test_resources_dir() else { return };
        let (dir, cache) = open_test_cache();
        add_test_book(dir.path(), &cache, "Untagged Book", "Author");

        let catalog_path = dir.path().join("catalog");
        let cache_dir = dir.path().join("cache");
        let mut opts = base_opts(catalog_path, cache_dir, resources_dir);
        opts.generate_genres = true;
        opts.only_genres_selected = true;

        let err = build_catalog(&cache, &opts).unwrap_err();
        assert!(matches!(err, CatalogError::EmptyCatalog));
    }

    #[test]
    fn build_catalog_generates_genre_pages_and_ncx_genre_section() {
        let Some(resources_dir) = test_resources_dir() else { return };
        let (dir, cache) = open_test_cache();
        let id = add_test_book(dir.path(), &cache, "Tagged Book", "Author");
        cache.set_field(id, "tags", "Fiction").unwrap();

        let catalog_path = dir.path().join("catalog");
        let cache_dir = dir.path().join("cache");
        let mut opts = base_opts(catalog_path, cache_dir, resources_dir);
        opts.generate_genres = true;
        opts.generate_authors = true;

        let out = build_catalog(&cache, &opts).unwrap();
        let genre_path = out.html_files.iter().find(|(p, _)| p.starts_with("content/Genre_"));
        assert!(genre_path.is_some());
        assert!(out.opf.contains("Genre_"));
        assert!(out.ncx.contains("genre-"));
    }

    #[test]
    fn build_catalog_generates_description_pages_without_covers() {
        let Some(resources_dir) = test_resources_dir() else { return };
        let (dir, cache) = open_test_cache();
        let id = add_test_book(dir.path(), &cache, "Described Book", "Author");
        cache.set_field(id, "comments", "A great read").unwrap();

        let catalog_path = dir.path().join("catalog");
        let cache_dir = dir.path().join("cache");
        let mut opts = base_opts(catalog_path, cache_dir, resources_dir);
        opts.generate_descriptions = true;

        let out = build_catalog(&cache, &opts).unwrap();
        let desc_path = out.html_files.iter().find(|(p, _)| p.starts_with("content/book_"));
        assert!(desc_path.is_some());
        assert!(desc_path.unwrap().1.contains("A great read"));
        // No cover and no default_cover_path -- no thumbnails generated.
        assert!(out.thumbs.is_empty());
    }

    #[test]
    fn build_catalog_copies_resources_when_the_reference_tree_is_present() {
        let Some(resources_dir) = test_resources_dir() else { return };
        let (dir, cache) = open_test_cache();
        add_test_book(dir.path(), &cache, "Book", "Author");

        let catalog_path = dir.path().join("catalog");
        let cache_dir = dir.path().join("cache");
        let opts = base_opts(catalog_path.clone(), cache_dir, resources_dir);

        build_catalog(&cache, &opts).unwrap();
        assert!(catalog_path.join("DefaultCover.jpg").exists());
        assert!(catalog_path.join("content/stylesheet.css").exists());
    }
}
