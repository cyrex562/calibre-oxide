//! Port of (a subset of) `calibre.srv.opds` -- the OPDS (Open Publication
//! Distribution System) Atom-feed catalog, letting any OPDS-compatible
//! e-reader app browse and download books from the library over the
//! network.
//!
//! # Phase 1 scope
//!
//! This is the first of several planned increments for issue #60 (36
//! files, ~12,300 lines -- calibre's entire hand-rolled async HTTP/
//! WebSocket content server). See the crate root doc for the overall
//! architecture decision (`axum`/`tokio` replace the custom event loop
//! wholesale) and which of the 36 files this increment does and doesn't
//! cover.
//!
//! Ported here: the root navigation feed (`GET /opds`, upstream's
//! `TopLevel`) and the two built-in "by title"/"newest" acquisition
//! feeds (`GET /opds/navcatalog/{which}`, upstream's `get_all_books`).
//!
//! Category browsing (`opds_category`/`opds_categorygroup`/
//! `get_navcatalog`) is also ported, restricted to
//! `calibre_db::categories`'s own narrowed category list --
//! `authors`/`tags`/`series`/`publisher`/`languages` (no composite
//! columns, no user categories, no hierarchical categories -- see that
//! module's own doc). Large categories (more than
//! `max_opds_ungrouped_items` items) are bucketed by first letter,
//! matching upstream's own fallback.
//!
//! `GET /opds/search/{query}` (`opds_search`) is also ported, backed by
//! `calibre_db::search::search` -- calibre's real search-query
//! language (`tag:foo AND author:bar`-style expressions), the same
//! engine behind the desktop UI's search bar. An unparseable query 404s,
//! matching upstream's own behavior.
//!
//! **Not yet ported**: multi-library support (`library_map`/
//! `library_broker.py`) -- this server is single-library for now, so
//! `TopLevel`'s per-library nav entries are omitted.

use std::collections::{BTreeMap, HashSet};

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

use calibre_db::categories::{self, Tag};
use calibre_ebooks::dom::{Dom, NodeId};
use calibre_ebooks::metadata::authors::authors_to_string;
use calibre_ebooks::oeb::transforms::jacket::{comments_to_html, fmt_sidx, rating_to_stars};

use crate::errors::ServerError;
use crate::utils::{http_date, Offsets};
use crate::AppState;

/// The five categories `calibre_db::categories::get_categories`
/// supports, with their OPDS-facing friendly names -- matches upstream's
/// own `field_metadata[category]['name']` lookup, narrowed to the same
/// fixed list `calibre_db::categories`'s own doc discloses.
const CATEGORY_NAMES: &[(&str, &str)] =
    &[("tags", "Tags"), ("authors", "Authors"), ("series", "Series"), ("publisher", "Publisher"), ("languages", "Languages")];

fn friendly_category_name(category: &str) -> &'static str {
    CATEGORY_NAMES.iter().find(|(k, _)| *k == category).map(|(_, v)| *v).unwrap_or("Unknown")
}

const ATOM_NS: &str = "http://www.w3.org/2005/Atom";
const DC_NS: &str = "http://purl.org/dc/terms/";
const OPDS_NS: &str = "http://opds-spec.org/2010/catalog";

fn set_attr(dom: &mut Dom, id: NodeId, name: &str, value: impl Into<String>) {
    dom.node_mut(id).attrs.insert(name.to_string(), value.into());
}

fn append_text(dom: &mut Dom, parent: NodeId, text: &str) {
    let t = dom.new_text(text);
    dom.append_child(parent, t);
}

fn el(dom: &mut Dom, parent: NodeId, tag: &str) -> NodeId {
    let id = dom.new_element(tag);
    dom.append_child(parent, id);
    id
}

fn text_el(dom: &mut Dom, parent: NodeId, tag: &str, text: &str) -> NodeId {
    let id = el(dom, parent, tag);
    append_text(dom, id, text);
    id
}

fn updated_text(dt: DateTime<Utc>) -> String {
    dt.format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
}

fn default_feed_title() -> String {
    "calibre-oxide Library".to_string()
}

/// Port of `NAVLINK`/its `rel`-specific partials.
fn nav_link(dom: &mut Dom, parent: NodeId, href: &str, rel: Option<&str>, title: Option<&str>) {
    let id = el(dom, parent, "link");
    set_attr(dom, id, "type", "application/atom+xml;type=feed;profile=opds-catalog");
    set_attr(dom, id, "href", href);
    if let Some(rel) = rel {
        set_attr(dom, id, "rel", rel);
    }
    if let Some(title) = title {
        set_attr(dom, id, "title", title);
    }
}

struct FeedLinks<'a> {
    up: Option<&'a str>,
    first: Option<&'a str>,
    last: Option<&'a str>,
    next: Option<&'a str>,
    previous: Option<&'a str>,
}

impl Default for FeedLinks<'_> {
    fn default() -> Self {
        FeedLinks { up: None, first: None, last: None, next: None, previous: None }
    }
}

/// Port of the `Feed` base class: the `<feed>` root with its
/// title/author/id/icon/updated/search-link/start-link, plus whichever
/// nav links (up/first/last/next/previous) apply.
fn new_feed(id: &str, updated: DateTime<Utc>, title: Option<&str>, subtitle: Option<&str>, links: FeedLinks) -> (Dom, NodeId) {
    let mut dom = Dom::empty();
    let root = dom.root;
    let feed = el(&mut dom, root, "feed");
    set_attr(&mut dom, feed, "xmlns", ATOM_NS);
    set_attr(&mut dom, feed, "xmlns:dc", DC_NS);
    set_attr(&mut dom, feed, "xmlns:opds", OPDS_NS);

    text_el(&mut dom, feed, "title", title.unwrap_or(&default_feed_title()));
    if let Some(subtitle) = subtitle {
        text_el(&mut dom, feed, "subtitle", subtitle);
    }
    let author = el(&mut dom, feed, "author");
    text_el(&mut dom, author, "name", "calibre-oxide");
    let uri = el(&mut dom, author, "uri");
    append_text(&mut dom, uri, "https://github.com/cyrex562/calibre-oxide");

    text_el(&mut dom, feed, "id", id);
    let icon = el(&mut dom, feed, "icon");
    append_text(&mut dom, icon, "/favicon.png");
    text_el(&mut dom, feed, "updated", &updated_text(updated));

    let search = el(&mut dom, feed, "link");
    set_attr(&mut dom, search, "type", "application/atom+xml");
    set_attr(&mut dom, search, "rel", "search");
    set_attr(&mut dom, search, "title", "Search");
    set_attr(&mut dom, search, "href", "/opds/search/{searchTerms}");

    nav_link(&mut dom, feed, "/opds", Some("start"), None);
    if let Some(up) = links.up {
        nav_link(&mut dom, feed, up, Some("up"), None);
    }
    if let Some(first) = links.first {
        nav_link(&mut dom, feed, first, Some("first"), None);
    }
    if let Some(last) = links.last {
        nav_link(&mut dom, feed, last, Some("last"), None);
    }
    if let Some(next) = links.next {
        nav_link(&mut dom, feed, next, Some("next"), Some("Next"));
    }
    if let Some(previous) = links.previous {
        nav_link(&mut dom, feed, previous, Some("previous"), None);
    }

    (dom, feed)
}

/// Port of `NAVCATALOG_ENTRY`.
fn navcatalog_entry(dom: &mut Dom, feed: NodeId, updated: DateTime<Utc>, title: &str, description: &str, href: &str) {
    let id_ = format!("calibre-navcatalog:{}", sha1_hex(href));
    let entry = el(dom, feed, "entry");
    text_el(dom, entry, "title", title);
    text_el(dom, entry, "id", &id_);
    text_el(dom, entry, "updated", &updated_text(updated));
    let content = el(dom, entry, "content");
    set_attr(dom, content, "type", "text");
    append_text(dom, content, description);
    nav_link(dom, entry, href, None, None);
}

/// Port of `hashlib.sha1(...).hexdigest()`'s one call site here
/// (`NAVCATALOG_ENTRY`'s `id_`). Std's `DefaultHasher` (SipHash) stands
/// in for SHA-1 -- upstream only uses this hash to build an opaque,
/// stable Atom `<id>` from a URL, not for anything security-sensitive,
/// so a different (non-cryptographic, unkeyed-in-effect-here) hash
/// algorithm produces an equally valid, just differently-valued, stable
/// id -- no client depends on matching upstream's exact digest.
fn sha1_hex(data: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    data.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Port of `ACQUISITION_ENTRY`.
fn acquisition_entry(dom: &mut Dom, feed: NodeId, book: &Value, updated: DateTime<Utc>) {
    let entry = el(dom, feed, "entry");
    text_el(dom, entry, "title", book["title"].as_str().unwrap_or("Unknown"));
    let author = el(dom, entry, "author");
    let authors: Vec<String> = book["authors"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
    text_el(dom, author, "name", &authors_to_string(&authors));
    let uuid = book["uuid"].as_str().unwrap_or("");
    text_el(dom, entry, "id", &format!("urn:uuid:{uuid}"));
    text_el(dom, entry, "updated", &updated_text(updated));

    let mut extra = String::new();
    if let Some(rating) = book["rating"].as_f64() {
        if rating > 0.0 {
            extra.push_str(&format!("RATING: {}<br />", rating_to_stars(Some(rating), true)));
        }
    }
    if let Some(tags) = book["tags"].as_array() {
        if !tags.is_empty() {
            let tag_str: Vec<&str> = tags.iter().filter_map(|v| v.as_str()).collect();
            extra.push_str(&format!("TAGS: {}<br />", xml_escape(&tag_str.join(", "))));
        }
    }
    if let Some(series) = book["series"].as_str() {
        if !series.is_empty() {
            let sidx = book["series_index"].as_f64().unwrap_or(1.0);
            extra.push_str(&format!("SERIES: {} [{}]<br />", xml_escape(series), fmt_sidx(Some(sidx), false)));
        }
    }
    if let Some(comments) = book["comments"].as_str() {
        if !comments.is_empty() {
            extra.push_str(&comments_to_html(comments));
        }
    }
    if !extra.is_empty() {
        let content = el(dom, entry, "content");
        set_attr(dom, content, "type", "xhtml");
        let div = el(dom, content, "div");
        set_attr(dom, div, "xmlns", "http://www.w3.org/1999/xhtml");
        // `extra` is a small, closed set of hand-built HTML fragments
        // (all inputs already XML-escaped above except comments_to_html,
        // which itself produces escaped HTML), so it's inserted as a
        // single raw fragment via a text node -- Dom's serializer
        // doesn't currently support re-parsing an HTML fragment inline.
        append_text(dom, div, &extra);
    }

    let book_id = book["id"].as_i64().unwrap_or(0);
    if let Some(available_formats) = book["available_formats"].as_array() {
        for fmt in available_formats.iter().filter_map(|v| v.as_str()) {
            let fmt_lower = fmt.to_lowercase();
            let Some(mime) = mime_guess::from_ext(&fmt_lower).first_raw() else { continue };
            let link = el(dom, entry, "link");
            set_attr(dom, link, "type", mime);
            set_attr(dom, link, "href", format!("/get/{}/{}", fmt_lower, book_id));
            set_attr(dom, link, "rel", "http://opds-spec.org/acquisition");
        }
    }
    for (what, rel) in [
        ("cover", "http://opds-spec.org/cover"),
        ("thumb", "http://opds-spec.org/thumbnail"),
        ("cover", "http://opds-spec.org/image"),
        ("thumb", "http://opds-spec.org/image/thumbnail"),
    ] {
        let link = el(dom, entry, "link");
        set_attr(dom, link, "type", "image/jpeg");
        set_attr(dom, link, "href", format!("/get/{what}/{book_id}"));
        set_attr(dom, link, "rel", rel);
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn finish(dom: Dom, last_modified: DateTime<Utc>) -> Response {
    let xml = format!("<?xml version='1.0' encoding='UTF-8'?>\n{}", dom.serialize(dom.root));
    let mut resp = xml.into_response();
    resp.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/atom+xml; charset=UTF-8"));
    if let Ok(v) = HeaderValue::from_str(&http_date(last_modified)) {
        resp.headers_mut().insert(header::LAST_MODIFIED, v);
    }
    resp
}

/// `GET /opds`. Port of `opds()`, minus the per-category nav entries
/// (see this module's doc).
pub async fn root(State(state): State<AppState>) -> Result<Response, ServerError> {
    let last_modified = tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        move || cache.last_modified()
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    let (mut dom, feed) = new_feed("urn:calibre:main", last_modified, None, Some("Books in your library"), FeedLinks::default());
    navcatalog_entry(&mut dom, feed, last_modified, "By Newest", "Books sorted by Date", "/opds/navcatalog/newest");
    navcatalog_entry(&mut dom, feed, last_modified, "By Title", "Books sorted by Title", "/opds/navcatalog/title");
    for &(category, name) in CATEGORY_NAMES {
        navcatalog_entry(
            &mut dom,
            feed,
            last_modified,
            &format!("By {name}"),
            &format!("Books sorted by {name}"),
            &format!("/opds/navcatalog/N{category}"),
        );
    }
    Ok(finish(dom, last_modified))
}

#[derive(Debug, Deserialize)]
pub struct NavCatalogQuery {
    #[serde(default)]
    offset: i64,
}

/// Port of `get_acquisition_feed`: fetches, sorts, and paginates the
/// books in `ids`, rendering an `AcquisitionFeed`. Shared by
/// [`navcatalog`]'s `title`/`newest` case and [`category`]'s
/// drill-into-one-tag case (upstream's own `get_all_books`/
/// `opds_category` both ultimately call `get_acquisition_feed` too).
#[allow(clippy::too_many_arguments)]
async fn acquisition_feed_for_ids(
    state: &AppState,
    ids: HashSet<i32>,
    sort_field: &str,
    ascending: bool,
    feed_id: &str,
    feed_title: &str,
    page_url: &str,
    up_url: &str,
    offset: i64,
) -> Result<Response, ServerError> {
    let max_items = state.opts.max_opds_items;
    let sort_field_owned = sort_field.to_string();

    let (rows, last_modified) = tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        move || -> anyhow::Result<(Vec<Value>, DateTime<Utc>)> {
            // `authors_as_string = false` -- `acquisition_entry` needs a
            // real array to re-join via `authors_to_string` (matching
            // upstream's own `mi.authors` list), not a pre-joined string.
            let mut rows = cache.get_data_as_dict(None, false, Some(&ids), false)?;
            rows.sort_by(|a, b| {
                let ka = sort_key_for(a, &sort_field_owned);
                let kb = sort_key_for(b, &sort_field_owned);
                if ascending {
                    ka.cmp(&kb)
                } else {
                    kb.cmp(&ka)
                }
            });
            let lm = cache.last_modified()?;
            Ok((rows, lm))
        }
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    if rows.is_empty() {
        return Err(ServerError::NotFound("No books found".to_string()));
    }

    let offsets = Offsets::new(offset, max_items, rows.len() as i64)?;
    let page: Vec<&Value> = rows[offsets.offset as usize..(offsets.slice_upper_bound.min(rows.len() as i64)) as usize].iter().collect();

    let mut links = FeedLinks { up: Some(up_url), first: Some(page_url), ..Default::default() };
    let last_link = format!("{page_url}?offset={}", offsets.last_offset);
    links.last = Some(&last_link);
    let prev_link = format!("{page_url}?offset={}", offsets.previous_offset);
    if offsets.offset > 0 {
        links.previous = Some(&prev_link);
    }
    let next_link = format!("{page_url}?offset={}", offsets.next_offset);
    if offsets.next_offset > -1 {
        links.next = Some(&next_link);
    }

    let (mut dom, feed) = new_feed(feed_id, last_modified, Some(feed_title), None, links);
    for book in &page {
        acquisition_entry(&mut dom, feed, book, last_modified);
    }
    Ok(finish(dom, last_modified))
}

fn sort_key_for(book: &Value, field: &str) -> String {
    book[field].as_str().map(str::to_lowercase).unwrap_or_default()
}

/// `GET /opds/navcatalog/{which}`. Port of `opds_navcatalog`: `title`/
/// `newest` dispatch to `get_all_books`-equivalent logic (inlined
/// below via [`acquisition_feed_for_ids`]); `N<category>` (upstream's
/// hex-encoded `N`-prefixed ids, simplified here to a plain
/// ASCII-category-name suffix -- see this module's doc for why the hex
/// encoding itself isn't replicated) dispatches to [`get_navcatalog`].
pub async fn navcatalog(State(state): State<AppState>, Path(which): Path<String>, Query(q): Query<NavCatalogQuery>) -> Result<Response, ServerError> {
    if which == "title" || which == "newest" {
        let sort_field = if which == "newest" { "timestamp" } else { "title" };
        let ascending = which == "title";
        let feed_title = format!("{} :: By {}", default_feed_title(), if which == "newest" { "Newest" } else { "Title" });
        let ids: HashSet<i32> = tokio::task::spawn_blocking({
            let cache = state.cache.clone();
            move || cache.all_book_ids()
        })
        .await
        .map_err(|e| ServerError::InternalServerError(e.to_string()))?
        .map_err(|e| ServerError::InternalServerError(e.to_string()))?
        .into_iter()
        .collect();
        let page_url = format!("/opds/navcatalog/{which}");
        return acquisition_feed_for_ids(&state, ids, sort_field, ascending, &format!("calibre-all:{sort_field}"), &feed_title, &page_url, "/opds", q.offset)
            .await;
    }
    let Some(category) = which.strip_prefix('N') else {
        return Err(ServerError::NotFound("Not found".to_string()));
    };
    get_navcatalog(&state, category, q.offset).await
}

/// Port of `CATALOG_ENTRY`, restricted to the `type_ == 'I'` case (a
/// real item with an id) -- upstream's `'N' + item.name` fallback for
/// composite-column pseudo-items has no id to fall back to here, since
/// `calibre_db::categories::Tag` doesn't track composite columns at
/// all (see that module's doc).
fn catalog_entry(dom: &mut Dom, feed: NodeId, tag: &Tag, category: &str, updated: DateTime<Utc>) {
    let id_ = format!("calibre:category:{}", tag.name);
    let href = format!("/opds/category/{category}/I{}", tag.id);
    let entry = el(dom, feed, "entry");
    text_el(dom, entry, "title", &tag.name);
    text_el(dom, entry, "id", &id_);
    text_el(dom, entry, "updated", &updated_text(updated));
    let content = el(dom, entry, "content");
    set_attr(dom, content, "type", "text");
    append_text(dom, content, &if tag.count == 1 { "one book".to_string() } else { format!("{} books", tag.count) });
    nav_link(dom, entry, &href, None, None);
}

/// Port of `CATALOG_GROUP_ENTRY`.
fn catalog_group_entry(dom: &mut Dom, feed: NodeId, letter: &str, count: usize, category: &str, updated: DateTime<Utc>) {
    let id_ = format!("calibre:category-group:{category}:{letter}");
    let href = format!("/opds/categorygroup/{category}/{letter}");
    let entry = el(dom, feed, "entry");
    text_el(dom, entry, "title", letter);
    text_el(dom, entry, "id", &id_);
    text_el(dom, entry, "updated", &updated_text(updated));
    let content = el(dom, entry, "content");
    set_attr(dom, content, "type", "text");
    append_text(dom, content, &if count == 1 { "one item".to_string() } else { format!("{count} items") });
    nav_link(dom, entry, &href, None, None);
}

/// First "letter" `catalog_group_entry`/`opds_categorygroup`'s prefix
/// matching group by -- upstream's own `val[0].upper()`, with the same
/// "no leading letter falls back to 'A'" upstream quirk preserved
/// (`if not val: val = 'A'`, reached when a name is empty).
fn group_letter(name: &str) -> String {
    name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "A".to_string())
}

/// Port of `get_navcatalog`: lists every item in `category`, either
/// directly (if there are few enough) or bucketed by first letter (if
/// there are more than `max_opds_ungrouped_items`).
async fn get_navcatalog(state: &AppState, category: &str, offset: i64) -> Result<Response, ServerError> {
    let category = category.to_string();
    let (categories, last_modified) = tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        let category = category.clone();
        move || -> anyhow::Result<(Vec<Tag>, DateTime<Utc>)> {
            let cats = categories::get_categories(&cache, "name", None)?;
            let items = cats.get(&category).cloned().unwrap_or_default();
            Ok((items, cache.last_modified()?))
        }
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    // A valid category with no items is legitimate (nothing 404s just
    // because e.g. no book has a publisher set); an unrecognized
    // category key is what upstream's own `if which not in categories`
    // guards against.
    if !CATEGORY_NAMES.iter().any(|(k, _)| *k == category) {
        return Err(ServerError::NotFound(format!("Category {category:?} not found")));
    }

    let category_name = friendly_category_name(&category);
    let feed_title = format!("{} :: By {category_name}", default_feed_title());
    let id_ = format!("calibre-category-feed:{category}");
    let page_url = format!("/opds/navcatalog/N{category}");
    let max_ungrouped = state.opts.max_opds_ungrouped_items;
    let max_items = state.opts.max_opds_items;

    if max_ungrouped > 0 && (categories.len() as i64) <= max_ungrouped {
        let offsets = Offsets::new(offset, max_items, categories.len() as i64)?;
        let page = &categories[offsets.offset as usize..(offsets.slice_upper_bound.min(categories.len() as i64)) as usize];
        let mut links = FeedLinks { up: Some("/opds"), first: Some(&page_url), ..Default::default() };
        let last_link = format!("{page_url}?offset={}", offsets.last_offset);
        links.last = Some(&last_link);
        let prev_link = format!("{page_url}?offset={}", offsets.previous_offset);
        if offsets.offset > 0 {
            links.previous = Some(&prev_link);
        }
        let next_link = format!("{page_url}?offset={}", offsets.next_offset);
        if offsets.next_offset > -1 {
            links.next = Some(&next_link);
        }
        let (mut dom, feed) = new_feed(&id_, last_modified, Some(&feed_title), None, links);
        for tag in page {
            catalog_entry(&mut dom, feed, tag, &category, last_modified);
        }
        Ok(finish(dom, last_modified))
    } else {
        let mut groups: BTreeMap<String, usize> = BTreeMap::new();
        for tag in &categories {
            *groups.entry(group_letter(&tag.name)).or_insert(0) += 1;
        }
        let group_items: Vec<(String, usize)> = groups.into_iter().collect();
        let offsets = Offsets::new(offset, max_items, group_items.len() as i64)?;
        let page = &group_items[offsets.offset as usize..(offsets.slice_upper_bound.min(group_items.len() as i64)) as usize];
        let mut links = FeedLinks { up: Some("/opds"), first: Some(&page_url), ..Default::default() };
        let last_link = format!("{page_url}?offset={}", offsets.last_offset);
        links.last = Some(&last_link);
        let prev_link = format!("{page_url}?offset={}", offsets.previous_offset);
        if offsets.offset > 0 {
            links.previous = Some(&prev_link);
        }
        let next_link = format!("{page_url}?offset={}", offsets.next_offset);
        if offsets.next_offset > -1 {
            links.next = Some(&next_link);
        }
        let (mut dom, feed) = new_feed(&id_, last_modified, Some(&feed_title), None, links);
        for (letter, count) in page {
            catalog_group_entry(&mut dom, feed, letter, *count, &category, last_modified);
        }
        Ok(finish(dom, last_modified))
    }
}

/// `GET /opds/category/{category}/{which}`. Port of `opds_category`'s
/// `type_ == 'I'` case (a specific item's books) -- upstream's
/// `"search"`/composite-column/`"news"` special cases don't apply to
/// this module's narrowed category list.
pub async fn category(State(state): State<AppState>, Path((category, which)): Path<(String, String)>, Query(q): Query<NavCatalogQuery>) -> Result<Response, ServerError> {
    let Some(id_str) = which.strip_prefix('I') else {
        return Err(ServerError::NotFound("Not found".to_string()));
    };
    let Ok(item_id) = id_str.parse::<i32>() else {
        return Err(ServerError::NotFound("Not found".to_string()));
    };

    let (ids, item_name) = tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        let category = category.clone();
        move || -> anyhow::Result<(HashSet<i32>, String)> {
            let ids = categories::book_ids_for_category_item(&cache, &category, item_id)?;
            let cats = categories::get_categories(&cache, "name", None)?;
            let name = cats.get(&category).and_then(|tags| tags.iter().find(|t| t.id == item_id)).map(|t| t.name.clone()).unwrap_or_default();
            Ok((ids, name))
        }
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|_| ServerError::NotFound(format!("Category {category:?} not found")))?;

    if ids.is_empty() {
        return Err(ServerError::NotFound("No books found".to_string()));
    }

    let sort_field = if category == "series" { "series" } else { "title" };
    let feed_title = format!("{} :: {}", default_feed_title(), item_name);
    let page_url = format!("/opds/category/{category}/{which}");
    let up_url = format!("/opds/navcatalog/N{category}");
    acquisition_feed_for_ids(&state, ids, sort_field, true, &format!("calibre-category:{category}:{item_id}"), &feed_title, &page_url, &up_url, q.offset).await
}

/// `GET /opds/categorygroup/{category}/{which}`. Port of
/// `opds_categorygroup`: every item in `category` whose name starts
/// with the letter `which`.
pub async fn categorygroup(State(state): State<AppState>, Path((category, which)): Path<(String, String)>, Query(q): Query<NavCatalogQuery>) -> Result<Response, ServerError> {
    let (items, last_modified) = tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        let category = category.clone();
        move || -> anyhow::Result<(Vec<Tag>, DateTime<Utc>)> {
            let cats = categories::get_categories(&cache, "name", None)?;
            Ok((cats.get(&category).cloned().unwrap_or_default(), cache.last_modified()?))
        }
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    let matching: Vec<&Tag> = items.iter().filter(|t| group_letter(&t.name) == which).collect();
    if matching.is_empty() {
        return Err(ServerError::NotFound(format!("No items in group {category:?}:{which:?}")));
    }

    let category_name = friendly_category_name(&category);
    let feed_title = format!("{} :: By {category_name} :: {which}", default_feed_title());
    let id_ = format!("calibre-category-group-feed:{category}:{which}");
    let page_url = format!("/opds/categorygroup/{category}/{which}");
    let up_url = format!("/opds/navcatalog/N{category}");
    let max_items = state.opts.max_opds_items;

    let offsets = Offsets::new(q.offset, max_items, matching.len() as i64)?;
    let page = &matching[offsets.offset as usize..(offsets.slice_upper_bound.min(matching.len() as i64)) as usize];
    let mut links = FeedLinks { up: Some(&up_url), first: Some(&page_url), ..Default::default() };
    let last_link = format!("{page_url}?offset={}", offsets.last_offset);
    links.last = Some(&last_link);
    let prev_link = format!("{page_url}?offset={}", offsets.previous_offset);
    if offsets.offset > 0 {
        links.previous = Some(&prev_link);
    }
    let next_link = format!("{page_url}?offset={}", offsets.next_offset);
    if offsets.next_offset > -1 {
        links.next = Some(&next_link);
    }
    let (mut dom, feed) = new_feed(&id_, last_modified, Some(&feed_title), None, links);
    for tag in page.iter() {
        catalog_entry(&mut dom, feed, tag, &category, last_modified);
    }
    Ok(finish(dom, last_modified))
}

/// `GET /opds/search/{query}`. Port of `opds_search`: runs `query`
/// through `calibre_db::search`'s real search-query-language engine
/// and renders the matches as an acquisition feed, sorted by title
/// (matching upstream's `get_acquisition_feed`'s own default
/// `sort_by='title'` -- `opds_search` doesn't override it).
pub async fn search(State(state): State<AppState>, Path(query): Path<String>, Query(q): Query<NavCatalogQuery>) -> Result<Response, ServerError> {
    let ids: HashSet<i32> = tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        let query = query.clone();
        move || calibre_db::search::search(&cache, &query)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|_| ServerError::NotFound(format!("Search: {query:?} not understood")))?
    .into_iter()
    .collect();

    let feed_title = default_feed_title();
    let feed_id = format!("calibre-search:{query}");
    let page_url = format!("/opds/search/{}", urlencoding::encode(&query));
    acquisition_feed_for_ids(&state, ids, "title", true, &feed_id, &feed_title, &page_url, "/opds", q.offset).await
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use calibre_db::cache::Cache;

    fn add_test_book(dir: &std::path::Path, cache: &Cache, title: &str, author: &str) -> i32 {
        let source = dir.join(format!("{title}.epub"));
        std::fs::write(&source, b"fake epub bytes").unwrap();
        let mut meta = calibre_ebooks::metadata::MetaInformation::default();
        meta.title = title.to_string();
        meta.authors = vec![author.to_string()];
        cache.add_book(&source, &meta).unwrap()
    }

    fn test_app(book_count: usize) -> (tempfile::TempDir, axum::Router) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        for i in 0..book_count {
            add_test_book(dir.path(), &cache, &format!("Book {i}"), "Author");
        }
        let state = crate::AppState { cache: std::sync::Arc::new(cache), opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None };
        let router = crate::test_router(state);
        (dir, router)
    }

    async fn get_body(router: &axum::Router, uri: &str) -> (StatusCode, String) {
        let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    #[tokio::test]
    async fn root_feed_is_atom_xml_with_the_two_builtin_nav_entries() {
        let (_dir, router) = test_app(1);
        let (status, body) = get_body(&router, "/opds").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.starts_with("<?xml"));
        assert!(body.contains("<feed"));
        assert!(body.contains("By Newest"));
        assert!(body.contains("By Title"));
        assert!(body.contains("/opds/navcatalog/newest"));
        assert!(body.contains("/opds/navcatalog/title"));
    }

    #[tokio::test]
    async fn navcatalog_title_lists_every_book_with_a_working_acquisition_link() {
        let (_dir, router) = test_app(2);
        let (status, body) = get_body(&router, "/opds/navcatalog/title").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("Book 0"));
        assert!(body.contains("Book 1"));
        assert!(body.contains("<name>Author</name>"), "author name missing from entry -- got: {body}");
        assert!(body.contains("http://opds-spec.org/acquisition"));
        assert!(body.contains("/get/epub/"));
    }

    #[tokio::test]
    async fn navcatalog_rejects_an_unknown_which() {
        let (_dir, router) = test_app(1);
        let (status, _) = get_body(&router, "/opds/navcatalog/bogus").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn navcatalog_404s_on_an_empty_library() {
        let (_dir, router) = test_app(0);
        let (status, _) = get_body(&router, "/opds/navcatalog/title").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn navcatalog_paginates_and_exposes_a_working_get_link_end_to_end() {
        let (_dir, router) = test_app(1);
        let (_, feed) = get_body(&router, "/opds/navcatalog/title").await;
        let href_start = feed.find("/get/epub/").unwrap();
        let href_rest = &feed[href_start..];
        let href_end = href_rest.find('"').unwrap();
        let href = &href_rest[..href_end];

        let (status, body) = get_body(&router, href).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.as_bytes(), b"fake epub bytes");
    }

    #[tokio::test]
    async fn root_feed_lists_the_five_category_nav_entries() {
        let (_dir, router) = test_app(1);
        let (_, body) = get_body(&router, "/opds").await;
        for (category, name) in [("tags", "Tags"), ("authors", "Authors"), ("series", "Series"), ("publisher", "Publisher"), ("languages", "Languages")] {
            assert!(body.contains(&format!("By {name}")), "missing nav entry for {name}");
            assert!(body.contains(&format!("/opds/navcatalog/N{category}")), "missing nav link for {category}");
        }
    }

    #[tokio::test]
    async fn navcatalog_by_authors_lists_authors_with_working_drill_down_links() {
        let (dir, cache) = {
            let dir = tempfile::tempdir().unwrap();
            let cache = Cache::new(dir.path()).unwrap();
            (dir, cache)
        };
        add_test_book(dir.path(), &cache, "Book A", "Jane Doe");
        add_test_book(dir.path(), &cache, "Book B", "John Smith");
        let state = crate::AppState { cache: std::sync::Arc::new(cache), opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None };
        let router = crate::test_router(state);

        let (status, body) = get_body(&router, "/opds/navcatalog/Nauthors").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("Jane Doe"));
        assert!(body.contains("John Smith"));
        assert!(body.contains("one book"));
        assert!(body.contains("/opds/category/authors/I"));
    }

    #[tokio::test]
    async fn navcatalog_rejects_an_unknown_category() {
        let (_dir, router) = test_app(1);
        let (status, _) = get_body(&router, "/opds/navcatalog/Nbogus").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn category_drill_down_lists_the_books_for_one_author_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        add_test_book(dir.path(), &cache, "Book A", "Jane Doe");
        add_test_book(dir.path(), &cache, "Book B", "John Smith");
        let state = crate::AppState { cache: std::sync::Arc::new(cache), opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None };
        let router = crate::test_router(state);

        let (_, navcat) = get_body(&router, "/opds/navcatalog/Nauthors").await;
        let href_start = navcat.find("/opds/category/authors/I").unwrap();
        let href_rest = &navcat[href_start..];
        let href = &href_rest[..href_rest.find('"').unwrap()];

        let (status, body) = get_body(&router, href).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("Book A") || body.contains("Book B"), "expected one author's book in: {body}");
        assert!(!(body.contains("Book A") && body.contains("Book B")), "expected only one author's books, got both: {body}");
    }

    #[tokio::test]
    async fn category_404s_for_an_unknown_item_id() {
        let (_dir, router) = test_app(1);
        let (status, _) = get_body(&router, "/opds/category/authors/I999999").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn categorygroup_lists_only_items_starting_with_the_given_letter() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        add_test_book(dir.path(), &cache, "Book A", "Jane Doe");
        add_test_book(dir.path(), &cache, "Book B", "John Smith");
        let state = crate::AppState { cache: std::sync::Arc::new(cache), opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None };
        let router = crate::test_router(state);

        let (status, body) = get_body(&router, "/opds/categorygroup/authors/J").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("Jane Doe"));
        assert!(body.contains("John Smith"), "both Jane Doe and John Smith start with J -- expected both, got: {body}");
    }

    #[tokio::test]
    async fn categorygroup_404s_when_no_items_match_the_letter() {
        let (dir, cache) = {
            let dir = tempfile::tempdir().unwrap();
            let cache = Cache::new(dir.path()).unwrap();
            (dir, cache)
        };
        add_test_book(dir.path(), &cache, "Book A", "Jane Doe");
        let state = crate::AppState { cache: std::sync::Arc::new(cache), opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None };
        let router = crate::test_router(state);

        let (status, _) = get_body(&router, "/opds/categorygroup/authors/Z").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn large_categories_are_bucketed_by_first_letter_instead_of_listed_directly() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        for i in 0..3 {
            add_test_book(dir.path(), &cache, &format!("Book {i}"), &format!("Author {i}"));
        }
        let mut opts = crate::opts::ServerOptions::default();
        opts.max_opds_ungrouped_items = 2; // force the letter-bucket branch with only 3 authors
        let state = crate::AppState { cache: std::sync::Arc::new(cache), opts: std::sync::Arc::new(opts), auth: None };
        let router = crate::test_router(state);

        let (status, body) = get_body(&router, "/opds/navcatalog/Nauthors").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("/opds/categorygroup/authors/"), "expected letter-group links, got: {body}");
        assert!(!body.contains("/opds/category/authors/I"), "expected no direct item links when bucketed, got: {body}");
    }

    #[tokio::test]
    async fn search_plain_word_matches_title_via_all_location() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        add_test_book(dir.path(), &cache, "Foundation", "Isaac Asimov");
        add_test_book(dir.path(), &cache, "Dune", "Frank Herbert");
        let state = crate::AppState { cache: std::sync::Arc::new(cache), opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None };
        let router = crate::test_router(state);

        let (status, body) = get_body(&router, "/opds/search/Foundation").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("Foundation"));
        assert!(!body.contains("Dune"), "search for Foundation should not match Dune, got: {body}");
        assert!(body.contains("http://opds-spec.org/acquisition"));
    }

    #[tokio::test]
    async fn search_location_prefix_matches_specific_field() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        add_test_book(dir.path(), &cache, "Foundation", "Isaac Asimov");
        add_test_book(dir.path(), &cache, "Dune", "Frank Herbert");
        let state = crate::AppState { cache: std::sync::Arc::new(cache), opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None };
        let router = crate::test_router(state);

        let (status, body) = get_body(&router, "/opds/search/author%3Aherbert").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("Dune"));
        assert!(!body.contains("Foundation"), "author:herbert should not match Asimov's book, got: {body}");
    }

    #[tokio::test]
    async fn search_with_no_matches_404s() {
        let (_dir, router) = test_app(1);
        let (status, _) = get_body(&router, "/opds/search/nosuchbookexists").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn search_unparseable_query_404s() {
        let (_dir, router) = test_app(1);
        let (status, _) = get_body(&router, "/opds/search/%28author%3Aasimov").await; // "(author:asimov" -- unmatched paren
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
