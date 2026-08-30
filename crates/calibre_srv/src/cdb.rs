//! Port of a subset of `calibre.srv.cdb` -- the write/mutation JSON API
//! calibre's own web-reader/book-list JS client uses to edit a library
//! (deleting books, changing metadata, replacing covers/formats), as
//! opposed to [`crate::ajax`]'s read-only endpoints.
//!
//! # Disclosed scope
//!
//! - `GET/POST /cdb/cmd/{which}/{version}` (`cdb_run`) -- the generic
//!   `calibredb`-CLI-over-HTTP dispatcher (`module_for_cmd`, running
//!   arbitrary `calibre.db.cli` command modules by name) is **not**
//!   ported. It would mean building a whole dynamic command registry
//!   equivalent to `calibre_db::cli`'s own module system, reachable
//!   over HTTP -- a large, separate undertaking, not attempted here.
//! - `POST /cdb/add-book/...` (`cdb_add_book`) is **not** ported --
//!   upstream sniffs real book metadata (title/authors/languages) out
//!   of arbitrary uploaded format bytes via `get_metadata`, which
//!   needs real per-format metadata *readers* wired into an HTTP
//!   upload path; this crate's own `Cache::add_book` instead takes a
//!   caller-supplied `MetaInformation` rather than deriving it, so
//!   there's no equivalent entry point to call from here yet.
//! - `POST /cdb/copy-to-library/...` (`cdb_copy_to_library`) is **not**
//!   ported -- needs real multi-library support (`library_map`), which
//!   nothing in this crate has yet (see `opds.rs`'s own doc for the
//!   same disclosed gap).
//!
//! **Ported here**, all requiring [`crate::auth::require_auth`] the
//! same way every other route in this crate does (this crate has no
//! separate `needs_db_write`/`restriction_for` write-access gate
//! distinct from ordinary auth -- a narrower model than upstream's,
//! disclosed rather than half-built):
//!
//! - `POST /cdb/delete-books/{book_ids}` -- `Cache::delete_book` per id.
//! - `POST /cdb/set-cover/{book_id}` -- raw image bytes in the request
//!   body, sniffed for a real JPEG/PNG magic number (matching
//!   upstream's own `imghdr.what` check in `set-fields`; `set-cover`
//!   itself doesn't validate upstream either, so this is stricter,
//!   disclosed as intentional rather than a gap).
//! - `POST /cdb/set-fields/{book_id}` -- `changes` dict of field ->
//!   new value, plus the `cover`/`added_formats`/`removed_formats`
//!   special keys upstream also handles. Clearing the cover (a `null`
//!   `cover` value) is not supported (this crate has no
//!   `clear_cover` -- only `covers::set_cover`); every other change
//!   upstream supports for these fields is real. Returns the same
//!   shape upstream does: `{book_id: full-book-json}` for every
//!   dirtied id (via [`crate::ajax::book_json`]).

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::Json;
use rand::Rng;
use serde::Deserialize;
use serde_json::Value;

use crate::ajax::{book_json, fetch_rows};
use crate::errors::ServerError;
use crate::AppState;

/// `POST /cdb/delete-books/{book_ids}`. Port of `cdb_delete_book`.
pub async fn delete_books(State(state): State<AppState>, Path(book_ids): Path<String>) -> Result<Json<Value>, ServerError> {
    let mut ids = Vec::new();
    for part in book_ids.split(',') {
        let Ok(id) = part.trim().parse::<i32>() else {
            return Err(ServerError::BadRequest(format!("invalid book_ids: {book_ids}")));
        };
        ids.push(id);
    }

    tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        move || -> anyhow::Result<()> {
            for id in ids {
                cache.delete_book(id)?;
            }
            Ok(())
        }
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    Ok(Json(serde_json::json!({})))
}

fn sniff_image_format(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpeg")
    } else if data.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        Some("png")
    } else {
        None
    }
}

/// `POST /cdb/set-cover/{book_id}`. Port of `cdb_set_cover`.
pub async fn set_cover(State(state): State<AppState>, Path(book_id): Path<i32>, body: Bytes) -> Result<Json<Value>, ServerError> {
    if sniff_image_format(&body).is_none() {
        return Err(ServerError::BadRequest("Cover data must be either JPEG or PNG".to_string()));
    }
    let data = body.to_vec();
    tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        move || calibre_db::covers::set_cover(&cache, book_id, &data)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    Ok(Json(serde_json::json!([book_id])))
}

#[derive(Debug, Deserialize)]
pub struct SetFieldsBody {
    changes: serde_json::Map<String, Value>,
    #[serde(default)]
    loaded_book_ids: Vec<i32>,
    #[serde(default)]
    all_dirtied: bool,
}

#[derive(Debug, Deserialize)]
struct AddedFormat {
    ext: String,
    data_url: String,
}

fn decode_data_url(data_url: &str) -> Result<Vec<u8>, ServerError> {
    use base64::Engine;
    let raw = data_url.rsplit_once(',').map(|(_, b64)| b64).unwrap_or(data_url);
    base64::engine::general_purpose::STANDARD.decode(raw).map_err(|_| ServerError::BadRequest("data is not valid base64 encoded data".to_string()))
}

/// Converts one JSON `changes` value into the string
/// `Cache::set_field` expects, matching the join separators
/// `Cache::set_field`'s own `set_many_to_many_field` calls use for
/// each multi-value field (` & ` for authors, `, ` for
/// tags/languages -- see that function's own call sites).
fn value_to_field_string(field: &str, value: &Value) -> Option<String> {
    match field {
        "authors" => value.as_array().map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(" & ")),
        "tags" | "languages" => value.as_array().map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", ")),
        "identifiers" => value.as_object().map(|m| m.iter().filter_map(|(k, v)| v.as_str().map(|v| format!("{k}:{v}"))).collect::<Vec<_>>().join(",")),
        "rating" => value.as_f64().map(|display_rating| ((display_rating * 2.0).round() as i64).to_string()),
        _ => match value {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            Value::Null => Some(String::new()),
            _ => None,
        },
    }
}

/// `POST /cdb/set-fields/{book_id}`. Port of `cdb_set_fields`.
pub async fn set_fields(State(state): State<AppState>, Path(book_id): Path<i32>, Json(body): Json<SetFieldsBody>) -> Result<Json<Value>, ServerError> {
    let SetFieldsBody { mut changes, loaded_book_ids, all_dirtied } = body;

    if let Some(cover) = changes.remove("cover") {
        match cover {
            Value::Null => {
                return Err(ServerError::BadRequest("clearing the cover via set-fields is not supported".to_string()));
            }
            Value::String(data_url) => {
                let data = decode_data_url(&data_url)?;
                if sniff_image_format(&data).is_none() {
                    return Err(ServerError::BadRequest("Cover data must be either JPEG or PNG".to_string()));
                }
                tokio::task::spawn_blocking({
                    let cache = state.cache.clone();
                    move || calibre_db::covers::set_cover(&cache, book_id, &data)
                })
                .await
                .map_err(|e| ServerError::InternalServerError(e.to_string()))?
                .map_err(|e| ServerError::InternalServerError(e.to_string()))?;
            }
            _ => return Err(ServerError::BadRequest("Invalid cover value".to_string())),
        }
    }

    if let Some(added) = changes.remove("added_formats") {
        let added: Vec<AddedFormat> = serde_json::from_value(added).map_err(|_| ServerError::BadRequest("Format has no extension".to_string()))?;
        for fmt in added {
            let data = decode_data_url(&fmt.data_url)?;
            let ext = fmt.ext.to_lowercase();
            if ext.is_empty() || ext.len() > 10 || !ext.chars().all(|c| c.is_ascii_alphanumeric()) {
                // `ext` is embedded directly into a filesystem path below
                // (both the temp file here and, inside `add_format`, the
                // book's own destination filename) -- an allowlisted,
                // alphanumeric-only extension rules out path traversal
                // (`../`) and absolute-path (`/etc/...`) payloads.
                return Err(ServerError::BadRequest("Format has an invalid extension".to_string()));
            }
            let tmp_name = format!("cdb-upload-{book_id}-{}.{}", rand::rng().random::<u64>(), ext);
            let tmp_path = std::env::temp_dir().join(tmp_name);
            std::fs::write(&tmp_path, &data).map_err(|e| ServerError::InternalServerError(e.to_string()))?;
            let result = tokio::task::spawn_blocking({
                let cache = state.cache.clone();
                let tmp_path = tmp_path.clone();
                move || cache.add_format(book_id, &tmp_path, &ext, true)
            })
            .await
            .map_err(|e| ServerError::InternalServerError(e.to_string()))?;
            let _ = std::fs::remove_file(&tmp_path);
            result.map_err(|e| ServerError::InternalServerError(e.to_string()))?;
        }
    }

    if let Some(removed) = changes.remove("removed_formats") {
        let removed: Vec<String> = serde_json::from_value(removed).map_err(|_| ServerError::BadRequest("removed_formats must be a list of format extensions".to_string()))?;
        tokio::task::spawn_blocking({
            let cache = state.cache.clone();
            move || -> anyhow::Result<()> {
                for fmt in removed {
                    cache.remove_format(book_id, &fmt)?;
                }
                Ok(())
            }
        })
        .await
        .map_err(|e| ServerError::InternalServerError(e.to_string()))?
        .map_err(|e| ServerError::InternalServerError(e.to_string()))?;
    }

    let mut field_updates = Vec::new();
    for (field, value) in &changes {
        let Some(string_value) = value_to_field_string(field, value) else {
            return Err(ServerError::BadRequest(format!("cannot set field {field:?} to {value}")));
        };
        field_updates.push((field.clone(), string_value));
    }
    if !field_updates.is_empty() {
        tokio::task::spawn_blocking({
            let cache = state.cache.clone();
            move || -> anyhow::Result<()> {
                for (field, value) in field_updates {
                    cache.set_field(book_id, &field, &value)?;
                }
                Ok(())
            }
        })
        .await
        .map_err(|e| ServerError::InternalServerError(e.to_string()))?
        // `Cache::set_field`'s only real failure mode here is an
        // unwritable field name (a client error, not a server one) --
        // see its own doc/match arms.
        .map_err(|e| ServerError::BadRequest(e.to_string()))?;
    }

    // Upstream's `all_ids = dirtied if all_dirtied else (dirtied &
    // loaded_book_ids); all_ids |= {book_id}` matters when `db.set_field`
    // can dirty *other* books too (e.g. a shared-author metadata edit
    // touching every book with that author). This endpoint's
    // `Cache::set_field(book_id, ...)` only ever touches the one
    // `book_id` in the path, so `dirtied` here is always exactly
    // `{book_id}` and the union/intersection dance always collapses to
    // the same result regardless of `all_dirtied`/`loaded_book_ids` --
    // both are still accepted (and validated) for request-shape
    // compatibility, they just can't change the answer in this port.
    let _ = (loaded_book_ids, all_dirtied);
    let rows = fetch_rows(&state, std::iter::once(book_id).collect()).await?;
    let mut ans = serde_json::Map::new();
    for row in rows {
        let id = row["id"].as_i64().unwrap_or(0);
        ans.insert(id.to_string(), book_json(&row));
    }
    Ok(Json(Value::Object(ans)))
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use calibre_db::cache::Cache;

    const JPEG_MAGIC: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F'];

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

    async fn post_bytes(router: &axum::Router, uri: &str, body: &[u8]) -> (StatusCode, serde_json::Value) {
        let req = Request::builder().method("POST").uri(uri).body(Body::from(body.to_vec())).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value = if body.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null) };
        (status, value)
    }

    #[tokio::test]
    async fn delete_books_removes_only_the_requested_book() {
        let (_dir, router) = test_app(2);
        let (status, body) = post_json(&router, "/cdb/delete-books/1", serde_json::json!(null)).await;
        assert_eq!(status, StatusCode::OK, "got: {body}");

        let (status, _) = get_json(&router, "/ajax/book/1").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, book2) = get_json(&router, "/ajax/book/2").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(book2["title"], "Book 1");
    }

    #[tokio::test]
    async fn delete_books_rejects_a_non_integer_id() {
        let (_dir, router) = test_app(1);
        let (status, _) = post_json(&router, "/cdb/delete-books/bogus", serde_json::json!(null)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn set_cover_accepts_real_jpeg_bytes() {
        let (_dir, router) = test_app(1);
        let (status, body) = post_bytes(&router, "/cdb/set-cover/1", JPEG_MAGIC).await;
        assert_eq!(status, StatusCode::OK, "got: {body}");
        assert_eq!(body, serde_json::json!([1]));
    }

    #[tokio::test]
    async fn set_cover_rejects_non_image_bytes() {
        let (_dir, router) = test_app(1);
        let (status, _) = post_bytes(&router, "/cdb/set-cover/1", b"not an image").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn set_fields_updates_title_and_returns_the_new_book_json() {
        let (_dir, router) = test_app(1);
        let (status, body) = post_json(&router, "/cdb/set-fields/1", serde_json::json!({"changes": {"title": "New Title"}})).await;
        assert_eq!(status, StatusCode::OK, "got: {body}");
        assert_eq!(body["1"]["title"], "New Title");

        let (_, book) = get_json(&router, "/ajax/book/1").await;
        assert_eq!(book["title"], "New Title");
    }

    #[tokio::test]
    async fn set_fields_updates_authors_and_tags_arrays() {
        let (_dir, router) = test_app(1);
        let (status, body) = post_json(
            &router,
            "/cdb/set-fields/1",
            serde_json::json!({"changes": {"authors": ["Jane Doe", "John Smith"], "tags": ["scifi", "classic"]}}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "got: {body}");
        assert_eq!(body["1"]["authors"], serde_json::json!(["Jane Doe", "John Smith"]));
        let mut tags: Vec<&str> = body["1"]["tags"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        tags.sort();
        assert_eq!(tags, vec!["classic", "scifi"]);
    }

    #[tokio::test]
    async fn set_fields_halves_the_display_rating_back_to_storage_scale() {
        let (_dir, router) = test_app(1);
        // Client sends the 0..5 display rating (what /ajax/book returns);
        // storage is 0..10 -- see value_to_field_string's "rating" arm.
        let (status, body) = post_json(&router, "/cdb/set-fields/1", serde_json::json!({"changes": {"rating": 4.0}})).await;
        assert_eq!(status, StatusCode::OK, "got: {body}");
        assert_eq!(body["1"]["rating"], 4.0);
    }

    #[tokio::test]
    async fn set_fields_sets_cover_from_base64_jpeg() {
        use base64::Engine;
        let (_dir, router) = test_app(1);
        let encoded = base64::engine::general_purpose::STANDARD.encode(JPEG_MAGIC);
        let (status, body) = post_json(&router, "/cdb/set-fields/1", serde_json::json!({"changes": {"cover": format!("data:image/jpeg;base64,{encoded}")}})).await;
        assert_eq!(status, StatusCode::OK, "got: {body}");
    }

    #[tokio::test]
    async fn set_fields_rejects_clearing_the_cover() {
        let (_dir, router) = test_app(1);
        let (status, _) = post_json(&router, "/cdb/set-fields/1", serde_json::json!({"changes": {"cover": null}})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn set_fields_adds_and_removes_formats() {
        use base64::Engine;
        let (_dir, router) = test_app(1);
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"fake pdf bytes");
        let (status, body) = post_json(
            &router,
            "/cdb/set-fields/1",
            serde_json::json!({"changes": {"added_formats": [{"ext": "pdf", "data_url": format!("data:application/pdf;base64,{encoded}")}]}}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "got: {body}");
        let formats: Vec<&str> = body["1"]["formats"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert!(formats.contains(&"pdf"), "expected pdf in formats, got: {formats:?}");
        assert!(formats.contains(&"epub"), "expected epub in formats, got: {formats:?}");

        let (status, body) = post_json(&router, "/cdb/set-fields/1", serde_json::json!({"changes": {"removed_formats": ["pdf"]}})).await;
        assert_eq!(status, StatusCode::OK, "got: {body}");
        let formats: Vec<&str> = body["1"]["formats"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert!(!formats.contains(&"pdf"), "expected pdf removed, got: {formats:?}");
    }

    #[tokio::test]
    async fn set_fields_rejects_an_unwritable_field() {
        let (_dir, router) = test_app(1);
        let (status, _) = post_json(&router, "/cdb/set-fields/1", serde_json::json!({"changes": {"db_id": 5}})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn set_fields_rejects_a_path_traversal_extension_in_added_formats() {
        // `ext` is embedded directly into a filesystem path (the temp
        // upload file here, and the book's own destination filename
        // inside `Cache::add_format`) -- a `../`-laden extension must
        // be rejected outright, not just neutralized downstream.
        use base64::Engine;
        let (dir, router) = test_app(1);
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"pwned");
        let evil_ext = "../../../../../../../../tmp/cdb-traversal-poc";
        let (status, body) = post_json(
            &router,
            "/cdb/set-fields/1",
            serde_json::json!({"changes": {"added_formats": [{"ext": evil_ext, "data_url": format!("data:application/octet-stream;base64,{encoded}")}]}}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "got: {body}");
        assert!(!std::path::Path::new("/tmp/cdb-traversal-poc").exists(), "path traversal payload escaped the intended directory");
        let _ = dir; // keep the temp library alive for the duration of the request above
    }
}
