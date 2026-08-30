//! Port of a subset of `calibre.srv.books` -- cross-device reading
//! position sync for the in-browser EPUB reader (`book-get/set-last-
//! read-position`), backed by `Cache::get_last_read_positions`/
//! `Cache::set_last_read_position`.
//!
//! # Scope
//!
//! Upstream's `books.py` is mostly about `render_book.py`'s in-browser
//! EPUB rendering pipeline (a background job queue that converts a
//! book to the reader's own HTML/JSON format, cached by content hash)
//! -- none of that exists in this crate, and isn't attempted here.
//! Only the two reading-position endpoints, which don't depend on any
//! of that machinery, are ported:
//!
//! - `GET /book-get-last-read-position/{library_id}/{which}`
//! - `POST /book-set-last-read-position/{library_id}/{book_id}/{fmt}`
//!   (upstream's route allows `{+fmt}`, a multi-segment capture; this
//!   crate's formats are always a single path segment, so a plain
//!   `{fmt}` is used instead -- no real format string needs the extra
//!   generality).
//!
//! `last_read.py`'s separate srv-wide "recently read across every
//! library" cache (a second, `srv-last-read.sqlite`-backed store,
//! consumed by `code.py`'s HTML homepage) is **not** ported --
//! `code.py` itself (the server's own HTML UI) isn't part of this
//! crate at all.
//!
//! # Login requirement, narrowed
//!
//! Upstream's `get_last_read_position` 404s for anonymous users
//! (`if not user: raise HTTPNotFound('login required for sync')`).
//! This crate's auth model is all-or-nothing across the whole router
//! (see `auth::require_auth`) -- there's no notion of "logged in on an
//! otherwise-anonymous server". So: when auth is enabled, every
//! request already carries a real [`crate::auth::AuthenticatedUser`]
//! (enforced upstream of these handlers) and the login check is moot;
//! when auth is disabled server-wide, both endpoints fall back to the
//! anonymous user id `"_"` (matching `Cache::set_last_read_position`'s
//! own default) rather than 404ing -- a deliberate simplification, not
//! an oversight.

use axum::extract::{Extension, Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::Value;

use crate::auth::AuthenticatedUser;
use crate::errors::ServerError;
use crate::AppState;

fn effective_user(user: Option<Extension<AuthenticatedUser>>) -> String {
    user.map(|Extension(AuthenticatedUser(name))| name).unwrap_or_else(|| "_".to_string())
}

/// `GET /book-get-last-read-position/{library_id}/{which}`. Port of
/// `get_last_read_position`. `which` is `book_id1-fmt1_book_id2-fmt2_...`
/// (matching upstream's own `which.split('_')` -- the docstring's
/// comma-separated description in the original doesn't match its own
/// code, so this follows the code).
pub async fn get_last_read_position(State(state): State<AppState>, Path((_library_id, which)): Path<(String, String)>, user: Option<Extension<AuthenticatedUser>>) -> Result<Json<Value>, ServerError> {
    let user = effective_user(user);
    let mut ans = serde_json::Map::new();
    for item in which.split('_') {
        let (book_id_str, fmt) = item.split_once('-').unwrap_or((item, ""));
        let Ok(book_id) = book_id_str.parse::<i32>() else {
            continue;
        };
        let key = format!("{book_id}:{fmt}");
        let positions = tokio::task::spawn_blocking({
            let cache = state.cache.clone();
            let fmt = fmt.to_string();
            let user = user.clone();
            move || cache.get_last_read_positions(book_id, &fmt, &user)
        })
        .await
        .map_err(|e| ServerError::InternalServerError(e.to_string()))?
        .map_err(|e| ServerError::InternalServerError(e.to_string()))?;
        ans.insert(key, Value::Array(positions));
    }
    Ok(Json(Value::Object(ans)))
}

#[derive(Debug, Deserialize)]
pub struct SetLastReadPositionBody {
    device: String,
    cfi: Option<String>,
    pos_frac: f64,
}

/// `POST /book-set-last-read-position/{library_id}/{book_id}/{fmt}`.
/// Port of `set_last_read_position`.
pub async fn set_last_read_position(
    State(state): State<AppState>,
    Path((_library_id, book_id, fmt)): Path<(String, i32, String)>,
    user: Option<Extension<AuthenticatedUser>>,
    Json(body): Json<SetLastReadPositionBody>,
) -> Result<(), ServerError> {
    let user = effective_user(user);
    let title_exists = tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        move || cache.field_for(book_id, "title")
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .is_some();
    if !title_exists {
        return Err(ServerError::book_not_found(book_id, "default"));
    }

    tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        let cfi = body.cfi.clone();
        move || cache.set_last_read_position(book_id, &fmt, &user, &body.device, cfi.as_deref(), None, body.pos_frac)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use calibre_db::cache::Cache;

    fn add_test_book(dir: &std::path::Path, cache: &Cache, title: &str) -> i32 {
        let source = dir.join(format!("{title}.epub"));
        std::fs::write(&source, b"fake epub bytes").unwrap();
        let mut meta = calibre_ebooks::metadata::MetaInformation::default();
        meta.title = title.to_string();
        meta.authors = vec!["Author".to_string()];
        cache.add_book(&source, &meta).unwrap()
    }

    fn test_app() -> (tempfile::TempDir, axum::Router, i32) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let book_id = add_test_book(dir.path(), &cache, "Book 0");
        let state = crate::AppState { cache: std::sync::Arc::new(cache), opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None };
        let router = crate::test_router(state);
        (dir, router, book_id)
    }

    async fn post_json(router: &axum::Router, uri: &str, body: serde_json::Value) -> (StatusCode, String) {
        let req = Request::builder().method("POST").uri(uri).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8_lossy(&body).into_owned())
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
    async fn set_then_get_round_trips_a_reading_position() {
        let (_dir, router, book_id) = test_app();
        let (status, _) = post_json(&router, &format!("/book-set-last-read-position/default/{book_id}/epub"), serde_json::json!({"device": "phone", "cfi": "/6/4", "pos_frac": 0.3})).await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = get_json(&router, &format!("/book-get-last-read-position/default/{book_id}-epub")).await;
        assert_eq!(status, StatusCode::OK);
        let key = format!("{book_id}:epub");
        let positions = body[&key].as_array().unwrap();
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0]["device"], "phone");
        assert_eq!(positions[0]["cfi"], "/6/4");
        assert_eq!(positions[0]["pos_frac"], 0.3);
    }

    #[tokio::test]
    async fn set_last_read_position_404s_for_an_unknown_book() {
        let (_dir, router, _book_id) = test_app();
        let (status, _) = post_json(&router, "/book-set-last-read-position/default/999/epub", serde_json::json!({"device": "phone", "cfi": "/6/4", "pos_frac": 0.3})).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn empty_cfi_clears_the_position() {
        let (_dir, router, book_id) = test_app();
        post_json(&router, &format!("/book-set-last-read-position/default/{book_id}/epub"), serde_json::json!({"device": "phone", "cfi": "/6/4", "pos_frac": 0.3})).await;
        post_json(&router, &format!("/book-set-last-read-position/default/{book_id}/epub"), serde_json::json!({"device": "phone", "cfi": null, "pos_frac": 0.0})).await;

        let (_, body) = get_json(&router, &format!("/book-get-last-read-position/default/{book_id}-epub")).await;
        let key = format!("{book_id}:epub");
        assert_eq!(body[&key].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn get_supports_multiple_book_format_pairs_in_one_request() {
        let (_dir, router, book_id) = test_app();
        post_json(&router, &format!("/book-set-last-read-position/default/{book_id}/epub"), serde_json::json!({"device": "phone", "cfi": "/6/4", "pos_frac": 0.3})).await;
        post_json(&router, &format!("/book-set-last-read-position/default/{book_id}/pdf"), serde_json::json!({"device": "phone", "cfi": "/2", "pos_frac": 0.1})).await;

        let which = format!("{book_id}-epub_{book_id}-pdf");
        let (status, body) = get_json(&router, &format!("/book-get-last-read-position/default/{which}")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body[format!("{book_id}:epub")].as_array().unwrap().len(), 1);
        assert_eq!(body[format!("{book_id}:pdf")].as_array().unwrap().len(), 1);
    }
}
