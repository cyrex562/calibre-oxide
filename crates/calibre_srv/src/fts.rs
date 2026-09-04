//! Port of `calibre.srv.fts` -- full-text search over indexed book
//! content, backed by `calibre_db`'s already-ported FTS5 engine
//! (issue #226's `Cache::fts`/`FtsConnection`, the same one behind the
//! CLI's `fts_search`/`fts_index` subcommands -- found via the same
//! "grep `calibre_db` for prior art before assuming a fresh
//! implementation is needed" discipline this whole epic has followed).
//!
//! # What's real here
//!
//! - `GET /fts/search` -- runs `query` through `Cache::fts().search`,
//!   grouping a per-book `title`/`authors` metadata cache the same
//!   way upstream's `add_metadata` does. `restriction` (upstream's
//!   virtual-library restriction string) is reinterpreted as an
//!   ordinary `calibre_db::search` query used to narrow the book-id
//!   set -- this crate has no virtual-library concept (same disclosed
//!   gap as `ajax::search`'s `vl` parameter), but a restriction string
//!   really is just a search-query string upstream also feeds through
//!   `db.search()`, so this is a real, working narrowing, not a stub.
//! - `POST /fts/disable` / `POST /fts/indexing` -- `Cache::
//!   set_fts_enabled`.
//! - `POST /fts/reindex` -- `"all"` dirties every existing format
//!   (`FtsConnection::dirty_existing`); a `{book_id: [fmt, ...]}` map
//!   dirties just those (`FtsConnection::dirty_book`).
//! - `GET /fts/snippets/{book_ids}` -- same query engine with
//!   `return_text=true`, grouped and deduplicated per book exactly
//!   like upstream's `output_results_as_text`-adjacent grouping (by
//!   whitespace-stripped text, preserving first-seen original text and
//!   collecting every format that produced an identical snippet).
//!
//! # Not ported
//!
//! - Upstream's `needs_db_write` distinction for `disable`/`reindex`/
//!   `indexing` -- same all-or-nothing auth model as every other route
//!   in this crate (see `cdb.rs`'s own doc for the precedent).
//! - A real background indexing pipeline (`fts_indexing_progress`'s
//!   `rate` field, `--wait-for-completion`) -- `calibre_db::fts`
//!   itself has none, see that module's own doc; `left`/`total` are
//!   real, `rate` was never part of this endpoint's response shape
//!   anyway (only the CLI printed it).

use std::collections::HashSet;

use axum::extract::{Path, Query, State};
use axum::Json;
use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::Value;

use crate::errors::ServerError;
use crate::AppState;

fn use_stemming(raw: Option<&str>) -> bool {
    raw != Some("n")
}

fn ensure_fts_enabled(cache: &calibre_db::cache::Cache) -> Result<(), ServerError> {
    if !cache.is_fts_enabled().map_err(|e| ServerError::InternalServerError(e.to_string()))? {
        return Err(ServerError::PreconditionRequired("Full text searching is not enabled on this library".to_string()));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    query: Option<String>,
    use_stemming: Option<String>,
    query_id: Option<String>,
    restriction: Option<String>,
}

/// `GET /fts/search`. Port of `fts_search`.
pub async fn search(State(state): State<AppState>, Query(q): Query<SearchQuery>) -> Result<Json<Value>, ServerError> {
    let query = q.query.filter(|s| !s.is_empty()).ok_or_else(|| ServerError::BadRequest("No search query specified".to_string()))?;
    let stem = use_stemming(q.use_stemming.as_deref());

    let result = tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        move || -> Result<Value, ServerError> {
            ensure_fts_enabled(&cache)?;
            let (left, total) = cache.fts_indexing_progress().map_err(|e| ServerError::InternalServerError(e.to_string()))?;

            let restrict_to: Option<HashSet<i32>> = match q.restriction.filter(|s| !s.is_empty()) {
                Some(r) => Some(calibre_db::search::search(&cache, &r).map_err(|e| ServerError::InternalServerError(e.to_string()))?.into_iter().collect()),
                None => None,
            };

            let results = cache.fts().search(&query, stem, None, None, restrict_to.as_ref(), false).map_err(|e| ServerError::UnprocessableEntity(e.to_string()))?;

            let mut metadata_cache = serde_json::Map::new();
            let mut out_results = Vec::new();
            for r in &results {
                let key = r.book_id.to_string();
                if !metadata_cache.contains_key(&key) {
                    let title = cache.field_for(r.book_id, "title").ok().flatten().unwrap_or_default();
                    let authors = cache.field_for(r.book_id, "authors").ok().flatten().unwrap_or_default();
                    metadata_cache.insert(key, serde_json::json!({ "title": title, "authors": authors }));
                }
                out_results.push(serde_json::json!({ "book_id": r.book_id, "format": r.format }));
            }

            let mut ans = serde_json::Map::new();
            ans.insert("metadata".into(), Value::Object(metadata_cache));
            ans.insert("indexing_status".into(), serde_json::json!({ "left": left, "total": total }));
            if let Some(qid) = q.query_id {
                ans.insert("query_id".into(), Value::String(qid));
            }
            ans.insert("results".into(), Value::Array(out_results));
            Ok(Value::Object(ans))
        }
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))??;

    Ok(Json(result))
}

/// `POST /fts/disable`. Port of `fts_disable`.
pub async fn disable(State(state): State<AppState>) -> Result<(), ServerError> {
    tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        move || -> anyhow::Result<()> {
            if cache.is_fts_enabled()? {
                cache.set_fts_enabled(false)?;
            }
            Ok(())
        }
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;
    Ok(())
}

/// `POST /fts/indexing`. Port of `fts_indexing`; the JSON body must be
/// a bare boolean (`true`/`false`), matching upstream's own
/// `isinstance(enable, bool)` check -- a non-boolean body is rejected
/// by the `Json<bool>` extractor itself before this handler runs.
pub async fn indexing(State(state): State<AppState>, Json(enable): Json<bool>) -> Result<(), ServerError> {
    tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        move || cache.set_fts_enabled(enable)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;
    Ok(())
}

/// `POST /fts/reindex`. Port of `fts_reindex`; the JSON body is either
/// the string `"all"` or a `{book_id: [fmt, ...]}` map.
pub async fn reindex(State(state): State<AppState>, Json(body): Json<Value>) -> Result<(), ServerError> {
    tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        move || -> Result<(), ServerError> {
            ensure_fts_enabled(&cache)?;
            let fts = cache.fts();
            match &body {
                Value::String(s) if s == "all" => {
                    fts.dirty_existing().map_err(|e| ServerError::InternalServerError(e.to_string()))?;
                }
                Value::Object(map) => {
                    for (book_id_str, fmts) in map {
                        let book_id: i32 = book_id_str.parse().map_err(|_| ServerError::BadRequest("Invalid book ids".to_string()))?;
                        let fmts: Vec<&str> = fmts.as_array().map(|a| a.iter().filter_map(|v| v.as_str()).collect()).unwrap_or_default();
                        fts.dirty_book(book_id, &fmts).map_err(|e| ServerError::InternalServerError(e.to_string()))?;
                    }
                }
                _ => return Err(ServerError::BadRequest("Invalid book ids".to_string())),
            }
            Ok(())
        }
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))??;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct SnippetsQuery {
    query: Option<String>,
    use_stemming: Option<String>,
    query_id: Option<String>,
    snippet_size: Option<usize>,
    highlight_start: Option<String>,
    highlight_end: Option<String>,
}

/// `GET /fts/snippets/{book_ids}`. Port of `fts_snippets`: same query
/// engine as [`search`] with `return_text=true`, grouped per book and
/// deduplicated by whitespace-stripped text (matching upstream's own
/// `re.sub(r'\s+', '', text)` dedup key), preserving the first-seen
/// original text and collecting every format that produced an
/// identical snippet.
pub async fn snippets(State(state): State<AppState>, Path(book_ids): Path<String>, Query(q): Query<SnippetsQuery>) -> Result<Json<Value>, ServerError> {
    let query = q.query.filter(|s| !s.is_empty()).ok_or_else(|| ServerError::BadRequest("No search query specified".to_string()))?;
    let bids: HashSet<i32> = book_ids
        .split(',')
        .map(|s| s.trim().parse::<i32>())
        .collect::<Result<_, _>>()
        .map_err(|_| ServerError::BadRequest("Invalid list of book ids".to_string()))?;
    let stem = use_stemming(q.use_stemming.as_deref());
    let snippet_size = q.snippet_size.unwrap_or(32);
    let start = q.highlight_start.unwrap_or_else(|| "\u{1c}".to_string());
    let end = q.highlight_end.unwrap_or_else(|| "\u{1e}".to_string());

    let result = tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        let bids = bids.clone();
        move || -> Result<Value, ServerError> {
            ensure_fts_enabled(&cache)?;
            let results = cache.fts().search(&query, stem, Some((&start, &end)), Some(snippet_size), Some(&bids), true).map_err(|e| ServerError::UnprocessableEntity(e.to_string()))?;

            let mut snippets: IndexMap<i32, IndexMap<String, (Vec<String>, String)>> = bids.iter().map(|&b| (b, IndexMap::new())).collect();
            for r in results {
                let text = r.text.unwrap_or_default();
                let key: String = text.chars().filter(|c| !c.is_whitespace()).collect();
                let per_book = snippets.entry(r.book_id).or_default();
                let entry = per_book.entry(key).or_insert_with(|| (Vec::new(), text.clone()));
                entry.0.push(r.format);
            }

            let mut out = serde_json::Map::new();
            for (book_id, groups) in snippets {
                let list: Vec<Value> = groups.into_values().map(|(formats, text)| serde_json::json!({ "formats": formats, "text": text })).collect();
                out.insert(book_id.to_string(), Value::Array(list));
            }

            let mut ans = serde_json::Map::new();
            if let Some(qid) = q.query_id {
                ans.insert("query_id".into(), Value::String(qid));
            }
            ans.insert("snippets".into(), Value::Object(out));
            Ok(Value::Object(ans))
        }
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))??;

    Ok(Json(result))
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

    fn test_app() -> (tempfile::TempDir, axum::Router, std::sync::Arc<Cache>) {
        let dir = tempfile::tempdir().unwrap();
        let cache = std::sync::Arc::new(Cache::new(dir.path()).unwrap());
        let state = crate::AppState { libraries: None, cache: cache.clone(), opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None, changes: crate::web_socket::new_change_broadcaster(), reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()), book_cache: std::sync::Arc::new(crate::books_cache::BookCache::open_temp()), jobs: std::sync::Arc::new(crate::jobs::JobsManager::new(4, std::time::Duration::from_secs(3600))), render_jobs: std::sync::Arc::new(crate::render_endpoints::RenderJobRegistry::new()) };
        let router = crate::test_router(state);
        (dir, router, cache)
    }

    async fn get_json(router: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value = if body.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null) };
        (status, value)
    }

    async fn post_json(router: &axum::Router, uri: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let req = Request::builder().method("POST").uri(uri).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value = if body.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null) };
        (status, value)
    }

    #[tokio::test]
    async fn search_requires_fts_to_be_enabled_first() {
        let (_dir, router, _cache) = test_app();
        let (status, _) = get_json(&router, "/fts/search?query=Rust").await;
        assert_eq!(status, StatusCode::PRECONDITION_REQUIRED);
    }

    #[tokio::test]
    async fn search_requires_a_query_param() {
        let (_dir, router, _cache) = test_app();
        let (status, _) = get_json(&router, "/fts/search").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn indexing_enables_fts_and_search_finds_indexed_text_with_metadata() {
        let (dir, router, cache) = test_app();
        let b1 = add_test_book(dir.path(), &cache, "Book A", "Isaac Asimov");
        let b2 = add_test_book(dir.path(), &cache, "Book B", "Frank Herbert");

        let (status, _) = post_json(&router, "/fts/indexing", serde_json::json!(true)).await;
        assert_eq!(status, StatusCode::OK);

        cache.fts().add_text(b1, "EPUB", 0.0, Some("This is a book about Rust programming."), "", 0, "", None).unwrap();
        cache.fts().add_text(b2, "EPUB", 0.0, Some("Python is a fine language too."), "", 0, "", None).unwrap();

        let (status, body) = get_json(&router, "/fts/search?query=Rust").await;
        assert_eq!(status, StatusCode::OK, "got: {body}");
        let results = body["results"].as_array().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["book_id"], b1);
        assert_eq!(body["metadata"][b1.to_string()]["title"], "Book A");
        assert_eq!(body["metadata"][b1.to_string()]["authors"], "Isaac Asimov");
    }

    #[tokio::test]
    async fn disable_turns_off_fts() {
        let (_dir, router, cache) = test_app();
        cache.set_fts_enabled(true).unwrap();
        let (status, _) = post_json(&router, "/fts/disable", serde_json::json!(null)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(!cache.is_fts_enabled().unwrap());
    }

    #[tokio::test]
    async fn reindex_all_marks_every_existing_format_dirty() {
        let (dir, router, cache) = test_app();
        let b1 = add_test_book(dir.path(), &cache, "Book A", "Author");
        cache.set_fts_enabled(true).unwrap();
        cache.fts().add_text(b1, "EPUB", 0.0, Some("indexed"), "", 0, "", None).unwrap();
        let (left_before, _) = cache.fts_indexing_progress().unwrap();
        assert_eq!(left_before, 0);

        let (status, _) = post_json(&router, "/fts/reindex", serde_json::json!("all")).await;
        assert_eq!(status, StatusCode::OK);
        let (left_after, _) = cache.fts_indexing_progress().unwrap();
        assert_eq!(left_after, 1);
    }

    #[tokio::test]
    async fn reindex_specific_book_dirties_only_that_book() {
        let (_dir, router, cache) = test_app();
        cache.set_fts_enabled(true).unwrap();

        let (status, _) = post_json(&router, "/fts/reindex", serde_json::json!({"5": ["EPUB", "PDF"]})).await;
        assert_eq!(status, StatusCode::OK);
        let (left, total) = cache.fts_indexing_progress().unwrap();
        assert_eq!(left, 2);
        assert_eq!(total, 2);
    }

    #[tokio::test]
    async fn reindex_rejects_a_non_integer_book_id() {
        let (_dir, router, cache) = test_app();
        cache.set_fts_enabled(true).unwrap();
        let (status, _) = post_json(&router, "/fts/reindex", serde_json::json!({"bogus": ["EPUB"]})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn snippets_groups_and_dedupes_matches_per_book() {
        let (dir, router, cache) = test_app();
        let b1 = add_test_book(dir.path(), &cache, "Book A", "Author");
        cache.set_fts_enabled(true).unwrap();
        cache.fts().add_text(b1, "EPUB", 0.0, Some("A chapter about Rust programming."), "", 0, "", None).unwrap();
        cache.fts().add_text(b1, "PDF", 0.0, Some("A chapter about Rust programming."), "", 0, "", None).unwrap();

        let (status, body) = get_json(&router, &format!("/fts/snippets/{b1}?query=Rust")).await;
        assert_eq!(status, StatusCode::OK, "got: {body}");
        let group = body["snippets"][b1.to_string()].as_array().unwrap();
        // Identical snippet text from two formats collapses into one
        // group listing both formats, matching upstream's dedup.
        assert_eq!(group.len(), 1);
        let formats = group[0]["formats"].as_array().unwrap();
        assert_eq!(formats.len(), 2);
    }

    #[tokio::test]
    async fn snippets_rejects_an_invalid_book_id_list() {
        let (_dir, router, cache) = test_app();
        cache.set_fts_enabled(true).unwrap();
        let (status, _) = get_json(&router, "/fts/snippets/bogus?query=Rust").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}
