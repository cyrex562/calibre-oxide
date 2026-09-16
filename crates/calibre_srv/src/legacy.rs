//! Port of `calibre.srv.legacy` (issue #430) -- calibre's older,
//! pre-`content-server`-rewrite HTTP API: a self-contained mobile HTML
//! book listing (`/mobile`), two compatibility redirects (`/browse`,
//! `/stanza`), and a `/legacy/get` format/cover download endpoint with
//! one old-Kindle-specific header quirk. Kept upstream, and here, for
//! old third-party clients that still expect these endpoint shapes --
//! the modern equivalents (`ajax.rs`/`opds.rs`/`cdb.rs`) already cover
//! everything a new client would use instead.
//!
//! # Disclosed narrowings
//!
//! - **`/mobile`'s per-book listing omits custom-column "extra text".**
//!   Upstream appends every displayable custom column's formatted
//!   value after the date/tags line
//!   (`ctx.is_field_displayable`/`field_metadata.ignorable_field_keys`),
//!   neither of which is ported anywhere in this crate yet. Title,
//!   series, authors, date, and tags -- the fields every book has --
//!   are all real. A real, separate, pre-existing gap, not invented
//!   for this issue.
//! - **`/static/*` and `/favicon.png` aren't implemented anywhere in
//!   this crate yet** (also pre-existing, also not this issue's
//!   scope). `/mobile`'s page correctly references those URLs
//!   (matching upstream's real path shape, `/static/{what}`), but they
//!   404 until that infrastructure exists -- the page itself is fully
//!   functional (search, sort, pagination, downloads all work), just
//!   unstyled and iconless in the meantime.
//! - **Plain string-templated HTML**, not a DOM tree -- this page's
//!   structure is fixed and never queried/mutated after being built,
//!   unlike `opds.rs`'s Atom feeds (which reuses `calibre_ebooks::dom`
//!   for exactly that reason). [`html_escape`] covers the same
//!   `clean_xml_chars`-driven safety upstream's own `E()`/`clean()`
//!   helpers provide.
//! - **Sort uses this crate's own established approximation of
//!   `multisort`**: real ICU collation via
//!   [`calibre_utils::icu::strcmp`] on one field (`opds.rs`'s
//!   [`crate::opds::sort_key_for`], reused directly here), not
//!   upstream's real multi-field `multisort`. Already disclosed where
//!   that helper is defined; not re-disclosed per caller.

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use chrono::Datelike;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;

use calibre_db::cache::Cache;
use calibre_db::field_metadata::FieldMetadata;
use calibre_ebooks::metadata::authors::authors_to_string;

use crate::errors::ServerError;
use crate::opds::sort_key_for;
use crate::utils::http_date;
use crate::AppState;

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn html_response(body: String) -> Response {
    let mut resp = format!("<!DOCTYPE html>\n{body}").into_response();
    resp.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=UTF-8"));
    resp
}

/// Port of `db.view.sanitize_sort_field_name`: resolve a search-term
/// alias to its real field key, then translate the two fields that
/// sort by a hidden proxy column rather than themselves.
fn sanitize_sort_field_name(fm: &FieldMetadata, field: &str) -> String {
    let key = fm.search_term_to_field_key(field.to_lowercase().trim()).to_string();
    match key.as_str() {
        "title" => "sort".to_string(),
        "authors" => "author_sort".to_string(),
        other => other.to_string(),
    }
}

/// Resolves the effective `(cache, library_id, library_map)` for a
/// request. Port of `get_library_data`'s non-strict mode (`/mobile`
/// never passes `strict_library_id`): an unknown `requested` id
/// silently falls back to the default library rather than 404ing.
/// `library_map` is empty in single-library mode
/// ([`AppState::libraries`] is `None`), matching upstream's own
/// `library_map` being trivial when there's only one library to list.
fn resolve_library(state: &AppState, requested: Option<&str>) -> (Arc<Cache>, String, indexmap::IndexMap<String, String>) {
    match &state.libraries {
        None => (state.cache.clone(), "default".to_string(), indexmap::IndexMap::new()),
        Some(broker) => {
            let map = broker.library_map();
            let library_id = requested.filter(|id| map.contains_key(*id)).map(|s| s.to_string()).unwrap_or_else(|| broker.default_library_id().to_string());
            let cache = broker.get(Some(&library_id)).unwrap_or_else(|| state.cache.clone());
            (cache, library_id, map)
        }
    }
}

// /mobile {{{

/// Port of `build_search_box`.
fn build_search_box(num: i64, search: &str, sort: &str, order: &str, library_id: &str) -> String {
    let mut num_opts = String::new();
    for option in [5, 10, 25, 100] {
        let selected = if option == num { " SELECTED" } else { "" };
        num_opts.push_str(&format!("<option value=\"{option}\"{selected}>{option}</option>"));
    }

    let mut sort_opts = String::new();
    for option in ["date", "author", "title", "rating", "size", "tags", "series"] {
        let selected = if option == sort { " SELECTED" } else { "" };
        sort_opts.push_str(&format!("<option value=\"{option}\"{selected}>{option}</option>"));
    }

    let mut order_opts = String::new();
    for option in ["ascending", "descending"] {
        let selected = if option == order { " SELECTED" } else { "" };
        order_opts.push_str(&format!("<option value=\"{option}\"{selected}>{option}</option>"));
    }

    let library_field = if library_id.is_empty() {
        String::new()
    } else {
        format!("<input name=\"library_id\" type=\"hidden\" value=\"{}\"/>", html_escape(library_id))
    };

    format!(
        "<div id=\"search_box\"><form method=\"get\" action=\"/mobile\" accept-charset=\"UTF-8\">Show \
         <select name=\"num\">{num_opts}</select> books matching \
         <input name=\"search\" id=\"s\" value=\"{}\"/> sorted by \
         <select name=\"sort\">{sort_opts}</select><select name=\"order\">{order_opts}</select>{library_field}\
         <input id=\"go\" type=\"submit\" value=\"Search\"/></form></div>",
        html_escape(search)
    )
}

/// Port of `build_navigation`.
fn build_navigation(start: i64, num: i64, total: i64, url_base: &str) -> String {
    let end = (start + num - 1).min(total);
    let mut left = String::new();
    let mut right = String::new();

    if start > 1 {
        for (t, s) in [("First", 1), ("Previous", (start - num).max(1))] {
            left.push_str(&format!("<a href=\"{url_base}&start={s}\">{t}</a>"));
        }
    }
    if total > start + num {
        for (t, s) in [("Next", start + num), ("Last", total - num + 1)] {
            right.push_str(&format!("<a href=\"{url_base}&start={s}\">{t}</a>"));
        }
    }

    format!(
        "<div class=\"navigation\"><span style=\"display: block; text-align: center;\">Books {start} to {end} of {total}</span>\
         <table class=\"buttons\"><tr><td class=\"button\" style=\"text-align:left\">{left}</td>\
         <td class=\"button\" style=\"text-align:right\">{right}</td></tr></table></div>"
    )
}

/// Port of `build_choose_library`.
fn build_choose_library(library_map: &indexmap::IndexMap<String, String>) -> String {
    let mut options = String::new();
    for (id, name) in library_map {
        options.push_str(&format!("<option value=\"{}\">{}</option>", html_escape(id), html_escape(name)));
    }
    format!(
        "<div id=\"choose_library\"><form method=\"GET\" action=\"/mobile\" accept-charset=\"UTF-8\">Change library to: \
         <select name=\"library_id\">{options}</select> <input type=\"submit\" value=\"Change library\"/></form></div>"
    )
}

/// Port of `build_index`. `books` are already-paginated rows from
/// `Cache::get_data_as_dict`.
#[allow(clippy::too_many_arguments)]
fn build_index(books: &[&Value], num: i64, search: &str, sort: &str, order: &str, start: i64, total: i64, url_base: &str, library_map: &indexmap::IndexMap<String, String>, library_id: &str) -> String {
    let search_box = build_search_box(num, search, sort, order, library_id);
    let navigation = build_navigation(start, num, total, url_base);

    let mut rows = String::new();
    for book in books {
        let book_id = book["id"].as_i64().unwrap_or(0);
        let title = book["title"].as_str().unwrap_or("Unknown");
        let authors: Vec<String> = book["authors"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
        let series = book["series"].as_str().unwrap_or("");
        let series_index = book["series_index"].as_f64().unwrap_or(1.0);
        let tags: Vec<&str> = book["tags"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).collect()).unwrap_or_default();

        let mut fmt_buttons = String::new();
        let formats: Vec<String> = book["available_formats"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).map(str::to_lowercase).collect()).unwrap_or_default();
        for fmt in &formats {
            if fmt.is_empty() || fmt.starts_with("original_") {
                continue;
            }
            fmt_buttons.push_str(&format!(
                "<span class=\"button\"><a href=\"/legacy/get/{fmt}/{book_id}/{}/{}.{fmt}\">{fmt}</a></span>",
                html_escape(library_id),
                html_escape(&title.chars().take(30).collect::<String>())
            ));
        }

        let series_text = if series.is_empty() { String::new() } else { format!("[{series} - {}]", fmt_series_index(series_index)) };
        let tags_text = if tags.is_empty() { String::new() } else { format!("Tags=[{}]", tags.join(", ")) };
        let date_text = book["timestamp"]
            .as_str()
            .and_then(|s| calibre_utils::date::parse_date(s, true))
            .filter(|dt| dt.year() != calibre_utils::date::UNDEFINED_DATE_YEAR)
            .map(|dt| dt.format("%d %b, %Y").to_string())
            .unwrap_or_default();

        let first_line = html_escape(&format!("{title} {series_text} by {}", authors_to_string(&authors)));
        let second_line = html_escape(&format!("{date_text} {tags_text}").trim());

        rows.push_str(&format!(
            "<tr><td><img type=\"image/jpeg\" border=\"0\" src=\"/get/thumb/{book_id}/{}\" class=\"thumbnail\"/></td>\
             <td>{fmt_buttons}<div class=\"data-container\"><span class=\"first-line\">{first_line}</span>\
             <span class=\"second-line\">{second_line}</span></div></td></tr>",
            html_escape(library_id)
        ));
    }

    let choose_library = if library_map.is_empty() { String::new() } else { build_choose_library(library_map) };

    format!(
        "<html><head><title>calibre Library</title>\
         <link rel=\"icon\" href=\"/favicon.png\" type=\"image/png\"/>\
         <link rel=\"stylesheet\" type=\"text/css\" href=\"/static/mobile.css\"/>\
         <link rel=\"apple-touch-icon\" href=\"/static/calibre.png\"/>\
         <meta name=\"robots\" content=\"noindex\"/></head>\
         <body><div id=\"logo\"><img src=\"/static/calibre.png\" alt=\"calibre\"/></div>\
         {search_box}{navigation}<hr class=\"spacer\"/><table id=\"listing\">{rows}</table>\
         <hr class=\"spacer\"/>{navigation}{choose_library}\
         <div style=\"text-align:center\"><a href=\"/\" style=\"text-decoration: none; color: blue\" \
         title=\"The full interface gives you many more features, but it may not work well on a small screen\">\
         Switch to the full interface (non-mobile interface)</a></div></body></html>"
    )
}

fn fmt_series_index(index: f64) -> String {
    if index.fract() == 0.0 {
        format!("{index:.0}")
    } else {
        format!("{index}")
    }
}

#[derive(Debug, Deserialize)]
pub struct MobileQuery {
    start: Option<String>,
    num: Option<String>,
    search: Option<String>,
    sort: Option<String>,
    order: Option<String>,
    library_id: Option<String>,
}

/// `GET /mobile`. Port of `mobile`/`build_index` -- a self-contained
/// mobile-friendly HTML book listing with search, sort, and
/// pagination. See this module's doc for the disclosed narrowings
/// (no custom-column extra text, `/static`/`/favicon.png` 404 for now).
pub async fn mobile(State(state): State<AppState>, Query(q): Query<MobileQuery>) -> Result<Response, ServerError> {
    let start: i64 = match q.start.as_deref() {
        None => 1,
        Some(s) => s.parse::<i64>().map_err(|_| ServerError::BadRequest("start is not an integer".to_string())).map(|v| v.max(1))?,
    };
    let num: i64 = match q.num.as_deref() {
        None => 25,
        Some(s) => s.parse::<i64>().map_err(|_| ServerError::BadRequest("num is not an integer".to_string())).map(|v| v.max(0))?,
    };
    let search = q.search.clone().unwrap_or_default();
    let ascending = q.order.as_deref().unwrap_or("").trim().eq_ignore_ascii_case("ascending");
    let order = if ascending { "ascending" } else { "descending" };

    let (cache, library_id, library_map) = resolve_library(&state, q.library_id.as_deref());

    let search_owned = search.clone();
    let sort_requested = q.sort.clone().unwrap_or_else(|| "date".to_string());
    let (rows, sort_by, last_modified) = tokio::task::spawn_blocking({
        let cache = cache.clone();
        move || -> anyhow::Result<(Vec<Value>, String, chrono::DateTime<chrono::Utc>)> {
            let ids: HashSet<i32> = calibre_db::search::search(&cache, &search_owned)?.into_iter().collect();
            let fm = FieldMetadata::from_cache(&cache)?;
            let sort_by = sanitize_sort_field_name(&fm, &sort_requested);
            let mut rows = cache.get_data_as_dict(None, false, Some(&ids), false)?;
            rows.sort_by(|a, b| {
                let ka = sort_key_for(a, &sort_by);
                let kb = sort_key_for(b, &sort_by);
                if ascending {
                    calibre_utils::icu::strcmp(&ka, &kb)
                } else {
                    calibre_utils::icu::strcmp(&kb, &ka)
                }
            });
            let last_modified = cache.last_modified()?;
            Ok((rows, sort_by, last_modified))
        }
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    let total = rows.len() as i64;
    let lower = ((start - 1).max(0) as usize).min(rows.len());
    let upper = (lower + num.max(0) as usize).min(rows.len());
    let books: Vec<&Value> = rows[lower..upper].iter().collect();

    let mut url_base = format!("/mobile?search={}&order={order}&sort={}&num={num}", urlencoding::encode(&search), urlencoding::encode(&sort_by));
    if !library_id.is_empty() {
        url_base.push_str(&format!("&library_id={}", urlencoding::encode(&library_id)));
    }
    let mut lm = library_map.clone();
    lm.shift_remove(&library_id);

    let body = build_index(&books, num, &search, &sort_by, order, start, total, &url_base, &lm, &library_id);
    let mut resp = html_response(body);
    if let Ok(v) = HeaderValue::from_str(&http_date(last_modified)) {
        resp.headers_mut().insert(header::LAST_MODIFIED, v);
    }
    Ok(resp)
}
// }}}

/// `GET /browse/{+rest}`. Port of `browse`: `book/{id}` redirects
/// (via a meta-refresh + JS page, matching upstream's own workaround
/// for old server book URLs -- https://bugs.launchpad.net/calibre/+bug/1698411)
/// to the modern book-details panel; anything else redirects to `/`.
pub async fn browse(Path(rest): Path<String>) -> Result<Response, ServerError> {
    if let Some(id) = rest.strip_prefix("book/") {
        let Ok(book_id) = id.parse::<i64>() else {
            return Err(ServerError::Redirect { location: "/".to_string(), permanent: false });
        };
        let redirect = format!("/#book_id={book_id}&panel=book_details");
        let body = format!(
            "<html><head><meta http-equiv=\"refresh\" content=\"0;url={redirect}\"/>\
             <script language=\"javascript\">window.location.href = \"{redirect}\"</script></head></html>"
        );
        return Ok(html_response(body));
    }
    Err(ServerError::Redirect { location: "/".to_string(), permanent: false })
}

/// `GET /browse` (no trailing segment -- axum's wildcard route can't
/// also match zero segments, so this covers the empty-`rest` case
/// [`browse`] itself handles above).
pub async fn browse_root() -> Result<Response, ServerError> {
    Err(ServerError::Redirect { location: "/".to_string(), permanent: false })
}

/// `GET /stanza`, `GET /stanza/{*rest}`. Port of `stanza`: the old
/// Stanza-reader OPDS entry point, now just a redirect to the real
/// one. `rest` is unused upstream too, so one handler covers both
/// route shapes.
pub async fn stanza() -> Result<Response, ServerError> {
    Err(ServerError::Redirect { location: "/opds".to_string(), permanent: false })
}

/// Shared body of `legacy_get`/`legacy_get_with_filename`. Port of
/// `legacy_get`: dispatches to the same logic as `/get`
/// ([`crate::content::handle`]), then strips `Content-Disposition` for
/// real old-Kindle browsers (`Kindle/3` in the User-Agent) -- upstream's
/// own disclosed reason: that header breaks downloads on those
/// browsers when the filename has non-ASCII characters
/// (https://www.mobileread.com/forums/showthread.php?t=364015). The
/// `filename` path segment itself is, as upstream's own handler body
/// shows, purely cosmetic in the URL -- never read here either.
async fn legacy_get_impl(state: AppState, what: String, book_id: String, library_id: String, headers: axum::http::HeaderMap) -> Result<Response, ServerError> {
    let is_old_kindle = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).map(|ua| ua.contains("Kindle/3")).unwrap_or(false);
    let mut resp = crate::content::handle(state, what, book_id, Some(&library_id)).await?;
    if is_old_kindle {
        resp.headers_mut().remove(header::CONTENT_DISPOSITION);
    }
    Ok(resp)
}

/// `GET /legacy/get/{what}/{book_id}/{library_id}` (no filename tail).
pub async fn legacy_get(State(state): State<AppState>, Path((what, book_id, library_id)): Path<(String, String, String)>, headers: axum::http::HeaderMap) -> Result<Response, ServerError> {
    legacy_get_impl(state, what, book_id, library_id, headers).await
}

/// `GET /legacy/get/{what}/{book_id}/{library_id}/{*filename}` -- same
/// handler, for the URL shape that includes upstream's cosmetic
/// filename tail (axum's wildcard segment needs its own route, since
/// it can't also match zero segments).
pub async fn legacy_get_with_filename(State(state): State<AppState>, Path((what, book_id, library_id, _filename)): Path<(String, String, String, String)>, headers: axum::http::HeaderMap) -> Result<Response, ServerError> {
    legacy_get_impl(state, what, book_id, library_id, headers).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn add_test_book(dir: &std::path::Path, cache: &Cache, title: &str, series: Option<(&str, f64)>, tags: &[&str]) -> i32 {
        let source = dir.join(format!("{title}.epub"));
        std::fs::write(&source, b"fake epub bytes").unwrap();
        let mut meta = calibre_ebooks::metadata::MetaInformation::default();
        meta.title = title.to_string();
        meta.authors = vec!["Author".to_string()];
        if let Some((_, idx)) = series {
            meta.series_index = idx;
        }
        let book_id = cache.add_book(&source, &meta).unwrap();
        // `add_book` only threads title/authors/series_index through
        // -- series NAME and tags are separate many-to-{one,many}
        // tables, set the same way any other caller would after the
        // fact (a real, pre-existing `add_book` narrowing, not
        // something this test works around specially).
        if let Some((s, _)) = series {
            cache.set_field(book_id, "series", s).unwrap();
        }
        if !tags.is_empty() {
            cache.set_field(book_id, "tags", &tags.join(", ")).unwrap();
        }
        book_id
    }

    fn test_app() -> (tempfile::TempDir, crate::AppState, axum::Router) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let state = crate::AppState {
            libraries: None,
            cache: Arc::new(cache),
            opts: Arc::new(crate::opts::ServerOptions::default()),
            auth: None,
            changes: crate::web_socket::new_change_broadcaster(),
            reader_profiles: Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()),
            book_cache: Arc::new(crate::books_cache::BookCache::open_temp()),
            jobs: Arc::new(crate::jobs::JobsManager::new(4, std::time::Duration::from_secs(3600))),
            render_jobs: Arc::new(crate::render_endpoints::RenderJobRegistry::new()),
            conversion_jobs: Arc::new(crate::convert::ConversionJobRegistry::new()), news_jobs: Arc::new(crate::news::NewsJobRegistry::new()), tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()),
        };
        let router = crate::test_router(state.clone());
        (dir, state, router)
    }

    #[tokio::test]
    async fn mobile_lists_books_and_renders_title_series_and_tags() {
        let (dir, state, router) = test_app();
        add_test_book(dir.path(), &state.cache, "First Book", Some(("A Series", 1.0)), &["fiction", "sample"]);

        let resp = router.oneshot(Request::builder().uri("/mobile").body(axum::body::Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get(header::CONTENT_TYPE).unwrap(), "text/html; charset=UTF-8");
        let body = String::from_utf8(to_bytes(resp.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();
        assert!(body.contains("First Book"), "{body}");
        assert!(body.contains("A Series - 1"), "{body}");
        assert!(body.contains("Tags=[fiction, sample]"), "{body}");
        assert!(body.contains("<!DOCTYPE html>"), "{body}");
    }

    #[tokio::test]
    async fn mobile_search_narrows_results() {
        let (dir, state, router) = test_app();
        add_test_book(dir.path(), &state.cache, "Alpha", None, &[]);
        add_test_book(dir.path(), &state.cache, "Beta", None, &[]);

        let resp = router.oneshot(Request::builder().uri("/mobile?search=Alpha").body(axum::body::Body::empty()).unwrap()).await.unwrap();
        let body = String::from_utf8(to_bytes(resp.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();
        assert!(body.contains("Alpha"), "{body}");
        assert!(!body.contains("Beta"), "{body}");
    }

    #[tokio::test]
    async fn mobile_pagination_respects_start_and_num() {
        let (dir, state, router) = test_app();
        for i in 1..=5 {
            add_test_book(dir.path(), &state.cache, &format!("Book {i}"), None, &[]);
        }

        let resp = router.oneshot(Request::builder().uri("/mobile?num=2&start=1&sort=title&order=ascending").body(axum::body::Body::empty()).unwrap()).await.unwrap();
        let body = String::from_utf8(to_bytes(resp.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();
        assert!(body.contains("Books 1 to 2 of 5"), "{body}");
        assert!(body.contains("Book 1"), "{body}");
        assert!(body.contains("Book 2"), "{body}");
        assert!(!body.contains("Book 3"), "{body}");
    }

    #[tokio::test]
    async fn mobile_rejects_a_non_integer_start() {
        let (_dir, _state, router) = test_app();
        let resp = router.oneshot(Request::builder().uri("/mobile?start=nope").body(axum::body::Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn browse_book_id_returns_a_redirect_page_pointing_at_the_modern_panel() {
        let (_dir, _state, router) = test_app();
        let resp = router.oneshot(Request::builder().uri("/browse/book/42").body(axum::body::Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = String::from_utf8(to_bytes(resp.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();
        assert!(body.contains("book_id=42"), "{body}");
        assert!(body.contains("panel=book_details"), "{body}");
    }

    #[tokio::test]
    async fn browse_anything_else_redirects_to_root() {
        let (_dir, _state, router) = test_app();
        let resp = router.oneshot(Request::builder().uri("/browse/somewhere-else").body(axum::body::Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(resp.headers().get(header::LOCATION).unwrap(), "/");
    }

    #[tokio::test]
    async fn browse_with_no_trailing_segment_redirects_to_root() {
        let (_dir, _state, router) = test_app();
        let resp = router.oneshot(Request::builder().uri("/browse").body(axum::body::Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(resp.headers().get(header::LOCATION).unwrap(), "/");
    }

    #[tokio::test]
    async fn stanza_redirects_to_opds() {
        let (_dir, _state, router) = test_app();
        let resp = router.oneshot(Request::builder().uri("/stanza/catalog").body(axum::body::Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(resp.headers().get(header::LOCATION).unwrap(), "/opds");
    }

    #[tokio::test]
    async fn legacy_get_serves_the_same_content_as_get() {
        let (dir, state, router) = test_app();
        let book_id = add_test_book(dir.path(), &state.cache, "Dl", None, &[]);

        let resp = router.oneshot(Request::builder().uri(format!("/legacy/get/epub/{book_id}/default/Dl.epub")).body(axum::body::Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"fake epub bytes");
    }

    #[tokio::test]
    async fn legacy_get_strips_content_disposition_for_old_kindle_browsers() {
        let (dir, state, router) = test_app();
        let book_id = add_test_book(dir.path(), &state.cache, "Dl", None, &[]);

        let resp = router
            .oneshot(
                Request::builder()
                    .uri(format!("/legacy/get/epub/{book_id}/default/Dl.epub"))
                    .header("User-Agent", "Mozilla/5.0 (Kindle/3.0)")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(resp.headers().get(header::CONTENT_DISPOSITION).is_none());
    }
}
