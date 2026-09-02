//! Port of a subset of `calibre.srv.ajax` -- the read-only JSON REST
//! API that calibre's own web-reader/book-list JS client (and any
//! other JSON-consuming client) uses to browse a library, as an
//! alternative to the Atom/OPDS feeds in [`crate::opds`].
//!
//! # Disclosed simplifications
//!
//! Upstream's `book_to_json`/`categories`/`category` are driven by a
//! full `field_metadata` system (custom columns, hierarchical
//! categories, user categories, per-field `is_category`/`is_csp`
//! introspection) this crate doesn't have (see
//! `calibre_db::categories`'s own doc for the same narrowing already
//! disclosed for the OPDS category feeds). This port covers:
//!
//! - `GET /ajax/book/{book_id}` / `GET /ajax/books?ids=...`: real book
//!   metadata as JSON, built from `Cache::get_data_as_dict`. No
//!   `category_urls` cross-referencing, no `id_is_uuid`, no
//!   `device_compatible`/`device_for_template` (device-specific upload
//!   path templating) -- all upstream features this crate's narrower
//!   data model can't drive yet.
//! - `GET /ajax/categories`: the same five standard categories
//!   `calibre_db::categories` supports. No icon URLs (no icon assets
//!   are part of this port), no "All books"/"Newest" pseudo-entries.
//! - `GET /ajax/category/{name}`: flat items only -- no subcategories,
//!   no hierarchical/user categories.
//! - `GET /ajax/books_in/{category}/{item}`: book ids for one category
//!   item, single-field sort only (upstream supports a comma-separated
//!   multi-field sort), no `get_additional_fields`.
//! - `GET /ajax/search`: same query language as `opds::search`, JSON
//!   shape instead of an Atom feed.
//! - `GET /ajax/library-info`: single-library only.

use std::collections::HashSet;

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::Value;

use calibre_db::categories;

use crate::errors::ServerError;
use crate::opds::sort_key_for;
use crate::AppState;

/// The one library id this single-library server exposes -- matches
/// `content.rs`'s own `"default"` convention (see
/// `ServerError::book_not_found`'s call sites).
const LIBRARY_ID: &str = "default";

fn get_pagination(num: Option<i64>, offset: Option<i64>) -> Result<(i64, i64), ServerError> {
    let num = num.unwrap_or(100);
    let offset = offset.unwrap_or(0);
    if num < 0 {
        return Err(ServerError::NotFound("Invalid num".to_string()));
    }
    if offset < 0 {
        return Err(ServerError::NotFound("Invalid offset".to_string()));
    }
    Ok((num, offset))
}

/// Port of `book_to_json`, narrowed per this module's doc. Reshapes
/// one `Cache::get_data_as_dict` row into the JSON API's own shape:
/// drops internal `fmt_<ext>` absolute paths in favor of `/get/...`
/// URLs (matching `opds::acquisition_entry`'s own URL scheme), and
/// halves `rating` (0..10 storage -> 0..5 display), matching
/// upstream's non-`device_compatible` path.
pub(crate) fn book_json(row: &Value) -> Value {
    let book_id = row["id"].as_i64().unwrap_or(0);
    let mut out = row.as_object().cloned().unwrap_or_default();
    out.remove("size");

    let rating = row["rating"].as_f64().map(|r| r / 2.0);
    out.insert("rating".into(), serde_json::json!(rating));

    out.insert("cover".into(), serde_json::json!(format!("/get/cover/{book_id}")));
    out.insert("thumbnail".into(), serde_json::json!(format!("/get/thumb/{book_id}")));

    let mut formats: Vec<String> = row["available_formats"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).map(str::to_lowercase).collect()).unwrap_or_default();
    formats.sort();
    out.remove("formats");
    for fmt in &formats {
        out.remove(&format!("fmt_{fmt}"));
    }
    let mut other_formats = serde_json::Map::new();
    let main_format = formats.first().map(|fmt| {
        let url = format!("/get/{fmt}/{book_id}");
        for other in formats.iter().skip(1) {
            other_formats.insert(other.clone(), serde_json::json!(format!("/get/{other}/{book_id}")));
        }
        serde_json::json!({ fmt.clone(): url })
    });
    out.insert("formats".into(), serde_json::json!(formats));
    out.insert("main_format".into(), main_format.unwrap_or(Value::Null));
    out.insert("other_formats".into(), Value::Object(other_formats));

    Value::Object(out)
}

pub(crate) async fn fetch_rows(state: &AppState, ids: HashSet<i32>) -> Result<Vec<Value>, ServerError> {
    tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        move || cache.get_data_as_dict(None, false, Some(&ids), false)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))
}

/// `GET /ajax/book/{book_id}`. Port of `book`, `id_is_uuid` not
/// supported (see module doc).
pub async fn book(State(state): State<AppState>, Path(book_id): Path<String>) -> Result<Json<Value>, ServerError> {
    let Ok(book_id) = book_id.parse::<i32>() else {
        return Err(ServerError::book_not_found(0, LIBRARY_ID));
    };
    let rows = fetch_rows(&state, std::iter::once(book_id).collect()).await?;
    let row = rows.into_iter().next().ok_or_else(|| ServerError::book_not_found(book_id, LIBRARY_ID))?;
    Ok(Json(book_json(&row)))
}

#[derive(Debug, Deserialize)]
pub struct BooksQuery {
    ids: Option<String>,
}

/// `GET /ajax/books?ids=1,2,3` (or `ids=all`/omitted). Port of
/// `books`. Ids that don't exist map to `null`, matching upstream's
/// own `ans[book_id] = None` for ids outside `allowed_book_ids`.
pub async fn books(State(state): State<AppState>, Query(q): Query<BooksQuery>) -> Result<Json<Value>, ServerError> {
    let requested: Option<HashSet<i32>> = match q.ids.as_deref() {
        None | Some("all") => None,
        Some(s) => {
            let mut set = HashSet::new();
            for part in s.split(',') {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                let Ok(id) = part.parse::<i32>() else {
                    return Err(ServerError::NotFound("ids must a comma separated list of integers".to_string()));
                };
                set.insert(id);
            }
            Some(set)
        }
    };

    let all_ids: HashSet<i32> = tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        move || cache.all_book_ids()
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .into_iter()
    .collect();

    let ids = requested.unwrap_or_else(|| all_ids.clone());
    let found: HashSet<i32> = ids.intersection(&all_ids).copied().collect();
    let rows = fetch_rows(&state, found).await?;

    let mut ans = serde_json::Map::new();
    for id in &ids {
        ans.insert(id.to_string(), Value::Null);
    }
    for row in rows {
        let id = row["id"].as_i64().unwrap_or(0);
        ans.insert(id.to_string(), book_json(&row));
    }
    Ok(Json(Value::Object(ans)))
}

/// `GET /ajax/categories`. Port of `categories`, narrowed per module
/// doc (no icons, no "All books"/"Newest" pseudo-entries).
pub async fn categories_list(State(state): State<AppState>) -> Result<Json<Value>, ServerError> {
    let cats = tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        move || categories::get_categories(&cache, "name", None)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    let mut ans: Vec<Value> = Vec::new();
    for &(key, name) in crate::opds::CATEGORY_NAMES {
        if cats.get(key).map(|v| v.is_empty()).unwrap_or(true) {
            continue;
        }
        ans.push(serde_json::json!({
            "url": format!("/ajax/category/{key}"),
            "name": name,
            "is_category": true,
        }));
    }
    Ok(Json(Value::Array(ans)))
}

#[derive(Debug, Deserialize)]
pub struct CategoryQuery {
    #[serde(default = "default_num")]
    num: i64,
    #[serde(default)]
    offset: i64,
    sort: Option<String>,
    sort_order: Option<String>,
}

fn default_num() -> i64 {
    100
}

fn ensure_val<'a>(v: Option<&'a str>, allowed: &[&'a str]) -> &'a str {
    match v {
        Some(x) if allowed.contains(&x) => x,
        _ => allowed[0],
    }
}

/// `GET /ajax/category/{name}`. Port of `category`, narrowed per
/// module doc: flat items only, no subcategories.
pub async fn category(State(state): State<AppState>, Path(name): Path<String>, Query(q): Query<CategoryQuery>) -> Result<Json<Value>, ServerError> {
    let (num, offset) = get_pagination(Some(q.num), Some(q.offset))?;
    let sort = ensure_val(q.sort.as_deref(), &["name", "rating", "popularity"]);
    let sort_order = ensure_val(q.sort_order.as_deref(), &["asc", "desc"]);

    let category_display_name = crate::opds::CATEGORY_NAMES.iter().find(|(k, _)| *k == name).map(|(_, n)| *n);
    let Some(category_display_name) = category_display_name else {
        return Err(ServerError::NotFound(format!("Category {name:?} not found")));
    };

    let mut cats = tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        let sort = sort.to_string();
        let name = name.clone();
        move || categories::get_categories(&cache, &sort, None).map(|m| m.get(&name).cloned().unwrap_or_default())
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    if sort_order == "desc" {
        cats.reverse();
    }
    let total_num = cats.len() as i64;
    let page: Vec<_> = cats.into_iter().skip(offset.max(0) as usize).take(num.max(0) as usize).collect();

    let items: Vec<Value> = page
        .iter()
        .map(|tag| {
            serde_json::json!({
                "name": tag.name,
                "average_rating": tag.avg_rating,
                "count": tag.count,
                "url": format!("/ajax/books_in/{name}/{}", tag.id),
                "has_children": false,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "category_name": category_display_name,
        "base_url": format!("/ajax/category/{name}"),
        "total_num": total_num,
        "offset": offset,
        "num": items.len(),
        "sort": sort,
        "sort_order": sort_order,
        "subcategories": Value::Array(vec![]),
        "items": items,
    })))
}

#[derive(Debug, Deserialize)]
pub struct BooksInQuery {
    #[serde(default = "default_num")]
    num: i64,
    #[serde(default)]
    offset: i64,
    sort: Option<String>,
    sort_order: Option<String>,
}

/// `GET /ajax/books_in/{category}/{item_id}`. Port of `books_in`,
/// narrowed per module doc: `item_id` must be the numeric category
/// item id `calibre_db::categories::Tag::id` (no `allbooks`/`newest`/
/// `search` pseudo-categories -- those are already covered by
/// `opds::navcatalog`'s `title`/`newest` and `ajax::search`), single
/// sort field, no `get_additional_fields`. In particular, a named
/// saved search (upstream's `dname == 'search'` case here) is just a
/// `search:name` query to [`search`] now that saved searches are real
/// (issue #422) -- nothing to add to *this* endpoint for that.
/// `GET /ajax/search?query=search%3Aname`.
pub async fn books_in(State(state): State<AppState>, Path((category, item_id)): Path<(String, String)>, Query(q): Query<BooksInQuery>) -> Result<Json<Value>, ServerError> {
    let (num, offset) = get_pagination(Some(q.num), Some(q.offset))?;
    let sort_field = q.sort.as_deref().unwrap_or("title");
    let sort_order = ensure_val(q.sort_order.as_deref(), &["asc", "desc"]);
    let Ok(item_id) = item_id.parse::<i32>() else {
        return Err(ServerError::NotFound(format!("Category id {item_id:?} not an integer")));
    };

    let ids = tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        let category = category.clone();
        move || categories::book_ids_for_category_item(&cache, &category, item_id)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|_| ServerError::NotFound(format!("Category {category:?} not found")))?;

    let mut rows = fetch_rows(&state, ids).await?;
    rows.sort_by(|a, b| {
        let ka = sort_key_for(a, sort_field);
        let kb = sort_key_for(b, sort_field);
        if sort_order == "asc" {
            ka.cmp(&kb)
        } else {
            kb.cmp(&ka)
        }
    });

    let total_num = rows.len() as i64;
    let page: Vec<i64> = rows.into_iter().skip(offset.max(0) as usize).take(num.max(0) as usize).map(|r| r["id"].as_i64().unwrap_or(0)).collect();

    Ok(Json(serde_json::json!({
        "total_num": total_num,
        "sort_order": sort_order,
        "offset": offset,
        "num": page.len(),
        "sort": sort_field,
        "base_url": format!("/ajax/books_in/{category}/{item_id}"),
        "book_ids": page,
    })))
}

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    query: Option<String>,
    #[serde(default = "default_num")]
    num: i64,
    #[serde(default)]
    offset: i64,
    sort: Option<String>,
    sort_order: Option<String>,
}

/// `GET /ajax/search?query=...`. Port of `search`/`search_result`,
/// single sort field (upstream supports a comma-separated multi-field
/// sort; not needed yet), no virtual-library restriction (`vl`, not
/// supported anywhere in this crate yet).
pub async fn search(State(state): State<AppState>, Query(q): Query<SearchQuery>) -> Result<Json<Value>, ServerError> {
    let (num, offset) = get_pagination(Some(q.num), Some(q.offset))?;
    let sort_field = q.sort.as_deref().unwrap_or("title");
    let sort_order = ensure_val(q.sort_order.as_deref(), &["asc", "desc"]);
    let query = q.query.unwrap_or_default();

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

    let mut rows = fetch_rows(&state, ids).await?;
    rows.sort_by(|a, b| {
        let ka = sort_key_for(a, sort_field);
        let kb = sort_key_for(b, sort_field);
        if sort_order == "asc" {
            ka.cmp(&kb)
        } else {
            kb.cmp(&ka)
        }
    });

    let total_num = rows.len() as i64;
    let page: Vec<i64> = rows.into_iter().skip(offset.max(0) as usize).take(num.max(0) as usize).map(|r| r["id"].as_i64().unwrap_or(0)).collect();

    Ok(Json(serde_json::json!({
        "total_num": total_num,
        "sort_order": sort_order,
        "offset": offset,
        "num": page.len(),
        "sort": sort_field,
        "base_url": "/ajax/search",
        "query": query,
        "library_id": LIBRARY_ID,
        "book_ids": page,
    })))
}

/// `GET /ajax/library-info`. Port of `library_info`, single-library
/// only.
pub async fn library_info() -> Json<Value> {
    Json(serde_json::json!({
        "library_map": { LIBRARY_ID: "calibre-oxide Library" },
        "default_library": LIBRARY_ID,
    }))
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
        let state = crate::AppState { cache: std::sync::Arc::new(cache), opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None, changes: crate::web_socket::new_change_broadcaster(), reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()) };
        let router = crate::test_router(state);
        (dir, router)
    }

    async fn get_json(router: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value = if body.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null) };
        (status, value)
    }

    #[tokio::test]
    async fn book_returns_real_metadata_with_get_urls() {
        let (_dir, router) = test_app(1);
        let (status, body) = get_json(&router, "/ajax/book/1").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["title"], "Book 0");
        assert_eq!(body["authors"], serde_json::json!(["Author"]));
        assert_eq!(body["cover"], "/get/cover/1");
        assert_eq!(body["thumbnail"], "/get/thumb/1");
        assert_eq!(body["main_format"], serde_json::json!({"epub": "/get/epub/1"}));
        assert!(body.get("size").is_none(), "size should be stripped like upstream, got: {body}");
    }

    #[tokio::test]
    async fn book_404s_for_an_unknown_id() {
        let (_dir, router) = test_app(1);
        let (status, _) = get_json(&router, "/ajax/book/999").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn books_maps_ids_to_metadata_and_unknown_ids_to_null() {
        let (_dir, router) = test_app(2);
        let (status, body) = get_json(&router, "/ajax/books?ids=1,999").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["1"]["title"], "Book 0");
        assert_eq!(body["999"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn books_all_returns_every_book() {
        let (_dir, router) = test_app(3);
        let (status, body) = get_json(&router, "/ajax/books").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.as_object().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn categories_list_includes_authors_and_omits_empty_categories() {
        let (_dir, router) = test_app(1);
        let (status, body) = get_json(&router, "/ajax/categories").await;
        assert_eq!(status, StatusCode::OK);
        let names: Vec<&str> = body.as_array().unwrap().iter().map(|c| c["url"].as_str().unwrap()).collect();
        assert!(names.contains(&"/ajax/category/authors"));
        assert!(!names.contains(&"/ajax/category/series"), "no book has a series -- should be omitted, got: {body}");
    }

    #[tokio::test]
    async fn category_lists_items_and_paginates() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        add_test_book(dir.path(), &cache, "Book A", "Jane Doe");
        add_test_book(dir.path(), &cache, "Book B", "John Smith");
        let state = crate::AppState { cache: std::sync::Arc::new(cache), opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None, changes: crate::web_socket::new_change_broadcaster(), reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()) };
        let router = crate::test_router(state);

        let (status, body) = get_json(&router, "/ajax/category/authors?num=1").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["total_num"], 2);
        assert_eq!(body["items"].as_array().unwrap().len(), 1);
        assert_eq!(body["items"][0]["count"], 1);
        assert!(body["items"][0]["url"].as_str().unwrap().starts_with("/ajax/books_in/authors/"));
    }

    #[tokio::test]
    async fn category_404s_for_an_unknown_category() {
        let (_dir, router) = test_app(1);
        let (status, _) = get_json(&router, "/ajax/category/bogus").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn books_in_lists_only_that_items_books() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        add_test_book(dir.path(), &cache, "Book A", "Jane Doe");
        add_test_book(dir.path(), &cache, "Book B", "John Smith");
        let state = crate::AppState { cache: std::sync::Arc::new(cache), opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None, changes: crate::web_socket::new_change_broadcaster(), reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()) };
        let router = crate::test_router(state);

        let (_, cat) = get_json(&router, "/ajax/category/authors").await;
        let item_url = cat["items"][0]["url"].as_str().unwrap().to_string();

        let (status, body) = get_json(&router, &item_url).await;
        assert_eq!(status, StatusCode::OK);
        let ids = body["book_ids"].as_array().unwrap();
        assert_eq!(ids.len(), 1);
    }

    #[tokio::test]
    async fn books_in_returns_an_empty_list_for_an_unknown_item_id() {
        // Matches upstream's own `books_in`: an unknown item id within a
        // *valid* category is just an empty result, not a 404 -- only an
        // unknown *category* name 404s (see the next test).
        let (_dir, router) = test_app(1);
        let (status, body) = get_json(&router, "/ajax/books_in/authors/999999").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["book_ids"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn books_in_404s_for_an_unknown_category() {
        let (_dir, router) = test_app(1);
        let (status, _) = get_json(&router, "/ajax/books_in/bogus/1").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn search_returns_matching_book_ids() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        add_test_book(dir.path(), &cache, "Foundation", "Isaac Asimov");
        add_test_book(dir.path(), &cache, "Dune", "Frank Herbert");
        let state = crate::AppState { cache: std::sync::Arc::new(cache), opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None, changes: crate::web_socket::new_change_broadcaster(), reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()) };
        let router = crate::test_router(state);

        let (status, body) = get_json(&router, "/ajax/search?query=Foundation").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["book_ids"], serde_json::json!([1]));
        assert_eq!(body["total_num"], 1);
    }

    #[tokio::test]
    async fn search_with_no_matches_returns_an_empty_list_not_404() {
        let (_dir, router) = test_app(1);
        let (status, body) = get_json(&router, "/ajax/search?query=nosuchbook").await;
        assert_eq!(status, StatusCode::OK, "ajax search should return an empty list, not 404, unlike opds::search");
        assert_eq!(body["book_ids"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn search_expands_a_saved_search_name_end_to_end() {
        // Real cross-crate wiring check for issue #422: a saved search
        // added via calibre_db::cache::Cache is resolvable through the
        // real HTTP endpoint via a `search:name` query, with no
        // srv-side code needed for it (see this endpoint's own doc).
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        add_test_book(dir.path(), &cache, "Foundation", "Isaac Asimov");
        add_test_book(dir.path(), &cache, "Dune", "Frank Herbert");
        cache.saved_search_add("asimov books", "title:Foundation").unwrap();
        let state = crate::AppState { cache: std::sync::Arc::new(cache), opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None, changes: crate::web_socket::new_change_broadcaster(), reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()) };
        let router = crate::test_router(state);

        let (status, body) = get_json(&router, "/ajax/search?query=search%3A%22asimov+books%22").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["book_ids"], serde_json::json!([1]));
    }

    #[tokio::test]
    async fn search_unparseable_query_404s() {
        let (_dir, router) = test_app(1);
        let (status, _) = get_json(&router, "/ajax/search?query=%28author%3Aasimov").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn library_info_reports_the_single_library() {
        let (_dir, router) = test_app(0);
        let (status, body) = get_json(&router, "/ajax/library-info").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["default_library"], "default");
        assert!(body["library_map"]["default"].is_string());
    }
}
