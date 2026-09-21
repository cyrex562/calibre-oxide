//! `GET /annotations/all` -- a real, **new** route (issue 1.8 of the
//! #816 epic).
//!
//! Upstream browses annotations from a Qt dialog, so there is no
//! `calibre.srv` endpoint to port. Per-book annotations were already
//! reachable (`/book-get-annotations`), but nothing could answer
//! "what have I highlighted across this whole library", which is the
//! question that makes annotations worth keeping.
//!
//! `calibre_db::annotations::all_annotations` was ported alongside
//! this: despite what that module's doc used to imply, it did not
//! exist before.

use std::collections::HashSet;

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::errors::ServerError;
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct AllAnnotationsQuery {
    /// `highlight` or `bookmark`. Omitted means both.
    #[serde(rename = "type")]
    pub annotation_type: Option<String>,
    /// Comma-separated book ids to restrict to.
    pub book_ids: Option<String>,
    pub limit: Option<usize>,
}

/// Hard ceiling regardless of what the client asks for. A library
/// with years of reading behind it can hold a great many highlights,
/// and an unbounded response would be slow to build and useless to
/// display.
const MAX_LIMIT: usize = 2000;

pub async fn all(State(state): State<AppState>, Query(q): Query<AllAnnotationsQuery>) -> Result<Json<Value>, ServerError> {
    if let Some(t) = &q.annotation_type {
        if t != "highlight" && t != "bookmark" {
            return Err(ServerError::BadRequest(format!("type must be highlight or bookmark (got {t:?})")));
        }
    }

    let restrict: Option<HashSet<i32>> = match &q.book_ids {
        Some(raw) if !raw.trim().is_empty() => {
            let mut ids = HashSet::new();
            for part in raw.split(',') {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                ids.insert(part.parse::<i32>().map_err(|_| ServerError::BadRequest(format!("invalid book id {part:?}")))?);
            }
            Some(ids)
        }
        _ => None,
    };

    let cache = state.cache.clone();
    let annotation_type = q.annotation_type.clone();
    let limit = q.limit.unwrap_or(MAX_LIMIT).min(MAX_LIMIT);

    let rows = tokio::task::spawn_blocking(move || {
        calibre_db::annotations::all_annotations(&cache, None, annotation_type.as_deref(), true, restrict.as_ref(), Some(limit))
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    // Titles make the list readable; the annotation rows only carry
    // book ids.
    let ids: HashSet<i32> = rows.iter().map(|r| r.book_id).collect();
    let cache = state.cache.clone();
    let titles = tokio::task::spawn_blocking(move || -> anyhow::Result<std::collections::HashMap<i32, String>> {
        if ids.is_empty() {
            return Ok(Default::default());
        }
        let books = cache.get_data_as_dict(None, true, Some(&ids), false)?;
        Ok(books
            .into_iter()
            .filter_map(|b| {
                let id = b.get("id")?.as_i64()? as i32;
                let title = b.get("title")?.as_str()?.to_string();
                Some((id, title))
            })
            .collect())
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    let out: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            json!({
                "id": r.id,
                "book_id": r.book_id,
                "title": titles.get(&r.book_id).cloned().unwrap_or_else(|| "Unknown".to_string()),
                "format": r.format,
                "text": r.text,
                "type": r.annotation.get("type").and_then(Value::as_str).unwrap_or(""),
                "timestamp": r.annotation.get("timestamp").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();

    Ok(Json(json!({ "count": out.len(), "annotations": out })))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use calibre_db::cache::Cache;
    use serde_json::Value;
    use tower::ServiceExt;

    fn app() -> (tempfile::TempDir, axum::Router) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        for (title, author) in [("One", "Ann"), ("Two", "Bo")] {
            let path = dir.path().join(format!("{title}.epub"));
            std::fs::write(&path, b"x").unwrap();
            let mut meta = calibre_ebooks::metadata::MetaInformation::default();
            meta.title = title.to_string();
            meta.authors = vec![author.to_string()];
            cache.add_book(&path, &meta).unwrap();
        }
        let highlight = serde_json::json!({"type": "highlight", "uuid": "h1", "timestamp": "2026-01-01T00:00:00Z", "highlighted_text": "a memorable line"});
        let bookmark = serde_json::json!({"type": "bookmark", "title": "Chapter 3", "timestamp": "2026-01-02T00:00:00Z"});
        calibre_db::annotations::merge_annotations_for_book(&cache, 1, "epub", &[highlight], "local", "viewer").unwrap();
        calibre_db::annotations::merge_annotations_for_book(&cache, 2, "epub", &[bookmark], "local", "viewer").unwrap();

        let state = crate::AppState {
            libraries: None,
            cache: std::sync::Arc::new(cache),
            opts: std::sync::Arc::new(crate::opts::ServerOptions::default()),
            auth: None,
            changes: crate::web_socket::new_change_broadcaster(),
            reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()),
            book_cache: std::sync::Arc::new(crate::books_cache::BookCache::open_temp()),
            jobs: std::sync::Arc::new(crate::jobs::JobsManager::new(4, std::time::Duration::from_secs(3600))),
            render_jobs: std::sync::Arc::new(crate::render_endpoints::RenderJobRegistry::new()),
            conversion_jobs: std::sync::Arc::new(crate::convert::ConversionJobRegistry::new()),
            news_jobs: std::sync::Arc::new(crate::news::NewsJobRegistry::new()),
            tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()),
            news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()),
            tts_voice: None,
            plugin_store: None,
            plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(calibre_customize::registry::PluginRegistry::new())),
        };
        (dir, crate::test_router(state))
    }

    async fn get(router: &axum::Router, uri: &str) -> (StatusCode, Value) {
        let resp = router.clone().oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn lists_every_annotation_with_its_book_title() {
        let (_d, router) = app();

        let (status, body) = get(&router, "/annotations/all").await;

        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["count"], 2);
        // The title is the whole reason this route joins against the
        // book rows -- annotation rows carry only a book id, and a
        // browser listing bare ids would be unusable.
        let titles: Vec<&str> = body["annotations"].as_array().unwrap().iter().map(|a| a["title"].as_str().unwrap()).collect();
        assert!(titles.contains(&"One"), "{body}");
        assert!(titles.contains(&"Two"), "{body}");
    }

    #[tokio::test]
    async fn filters_by_type() {
        let (_d, router) = app();

        let (_, body) = get(&router, "/annotations/all?type=highlight").await;

        assert_eq!(body["count"], 1);
        assert_eq!(body["annotations"][0]["text"], "a memorable line");
    }

    #[tokio::test]
    async fn restricts_to_given_books() {
        let (_d, router) = app();

        let (_, body) = get(&router, "/annotations/all?book_ids=2").await;

        assert_eq!(body["count"], 1);
        assert_eq!(body["annotations"][0]["book_id"], 2);
    }

    #[tokio::test]
    async fn an_unknown_type_is_refused_rather_than_silently_matching_nothing() {
        let (_d, router) = app();

        let (status, _) = get(&router, "/annotations/all?type=scribble").await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_non_numeric_book_id_is_refused() {
        let (_d, router) = app();
        assert_eq!(get(&router, "/annotations/all?book_ids=abc").await.0, StatusCode::BAD_REQUEST);
    }

    /// An unbounded response over a library with years of reading
    /// behind it would be slow to build and useless to display.
    #[tokio::test]
    async fn a_limit_is_honoured() {
        let (_d, router) = app();
        assert_eq!(get(&router, "/annotations/all?limit=1").await.1["count"], 1);
    }
}
