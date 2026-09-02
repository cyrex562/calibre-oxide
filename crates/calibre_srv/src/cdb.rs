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
//!
//! **Ported here**, all requiring [`crate::auth::require_auth`] the
//! same way every other route in this crate does (this crate has no
//! separate `needs_db_write`/`restriction_for` write-access gate
//! distinct from ordinary auth -- a narrower model than upstream's,
//! disclosed rather than half-built):
//!
//! - `POST /cdb/add-book/{job_id}/{add_duplicates}/{filename}/{library_id}`
//!   -- the uploaded file's raw bytes are the request body. Real
//!   metadata sniffing (title/authors/languages) via
//!   `calibre_ebooks::metadata::get_metadata`, dispatched by
//!   `filename`'s extension across the dozens of already-ported
//!   per-format readers (issue #424 -- the "needs new metadata-
//!   sniffing infra" assumption used to defer this in an earlier
//!   phase turned out to be wrong once actually checked: that
//!   dispatcher already existed). Real duplicate detection via
//!   `calibre_db::copy_to_library::find_duplicate_books`, the same
//!   author-intersection algorithm `copy-to-library`'s own duplicate
//!   check already uses. Not ported: `run_import_plugins` (upstream's
//!   plugin-driven pre-import format conversion -- no plugin system
//!   in this crate; the uploaded bytes are used as-is).
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
//! - `POST /cdb/copy-to-library/{target_library_id}/{library_id}`
//!   (issue #425, built on #423's `AppState::libraries`/`cache_for`)
//!   -- real copy/move between two open libraries with same-author/
//!   near-same-title duplicate detection via
//!   `calibre_db::copy_to_library::copy_one_book`. Narrower than
//!   upstream: no `add_formats_to_existing` automerge (rejected with
//!   a real error rather than silently downgraded to something else
//!   -- `copy_one_book` itself doesn't support it, see that module's
//!   doc), and `preserve_date`/`automerge_action` aren't accepted at
//!   all rather than accepted-but-inert.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::Json;
use calibre_utils::filenames::sanitize_file_name;
use rand::Rng;
use serde::Deserialize;
use serde_json::Value;

use crate::ajax::{book_json, fetch_rows};
use crate::errors::ServerError;
use crate::web_socket::{self, ChangeEvent};
use crate::AppState;

fn valid_extension(ext: &str) -> bool {
    !ext.is_empty() && ext.len() <= 10 && ext.chars().all(|c| c.is_ascii_alphanumeric())
}

/// `POST /cdb/add-book/{job_id}/{add_duplicates}/{filename}/{library_id}`.
/// Port of `cdb_add_book`.
pub async fn add_book(State(state): State<AppState>, Path((job_id, add_duplicates, filename, _library_id)): Path<(String, String, String, String)>, body: Bytes) -> Result<Json<Value>, ServerError> {
    if filename.is_empty() {
        return Err(ServerError::BadRequest("An empty filename is not allowed".to_string()));
    }
    let sanitized = sanitize_file_name(&filename);
    let ext = sanitized.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
    if !valid_extension(&ext) {
        return Err(ServerError::BadRequest("A filename with no extension is not allowed".to_string()));
    }
    let add_duplicates = add_duplicates == "y" || add_duplicates == "1";

    let tmp_path = std::env::temp_dir().join(format!("cdb-add-book-{}.{}", rand::rng().random::<u64>(), ext));
    tokio::fs::write(&tmp_path, &body).await.map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    let result = tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        let tmp_path = tmp_path.clone();
        let job_id = job_id.clone();
        let filename = filename.clone();
        move || -> Result<Value, ServerError> {
            let meta = calibre_ebooks::metadata::get_metadata(&tmp_path).map_err(|e| ServerError::BadRequest(format!("Could not read metadata from {filename:?}: {e}")))?;

            if !add_duplicates {
                let dups = calibre_db::copy_to_library::find_duplicate_books(&cache, &meta.title, &meta.authors).map_err(|e| ServerError::InternalServerError(e.to_string()))?;
                if !dups.is_empty() {
                    let duplicates: Vec<Value> = dups
                        .iter()
                        .filter_map(|id| calibre_db::copy_to_library::book_title_and_authors(&cache, *id).ok())
                        .map(|(title, authors)| serde_json::json!({ "title": title, "authors": authors }))
                        .collect();
                    return Ok(serde_json::json!({
                        "title": meta.title, "authors": meta.authors, "languages": meta.languages,
                        "filename": filename, "id": job_id, "duplicates": duplicates,
                    }));
                }
            }

            let book_id = cache.add_book(&tmp_path, &meta).map_err(|e| ServerError::InternalServerError(e.to_string()))?;
            Ok(serde_json::json!({
                "title": meta.title, "authors": meta.authors, "languages": meta.languages,
                "filename": filename, "id": job_id, "book_id": book_id,
            }))
        }
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))??;

    let _ = tokio::fs::remove_file(&tmp_path).await;

    if let Some(book_id) = result.get("book_id").and_then(|v| v.as_i64()) {
        web_socket::publish(&state, ChangeEvent::BooksAdded { book_ids: vec![book_id as i32] });
    }

    Ok(Json(result))
}

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
        let ids = ids.clone();
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

    web_socket::publish(&state, ChangeEvent::BooksDeleted { book_ids: ids });
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

    web_socket::publish(&state, ChangeEvent::MetadataChanged { book_ids: vec![book_id] });
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
    web_socket::publish(&state, ChangeEvent::MetadataChanged { book_ids: vec![book_id] });
    Ok(Json(Value::Object(ans)))
}

#[derive(Debug, Deserialize)]
pub struct CopyToLibraryBody {
    book_ids: Vec<i32>,
    #[serde(default, rename = "move")]
    move_books: bool,
    #[serde(default = "default_duplicate_action")]
    duplicate_action: String,
}

fn default_duplicate_action() -> String {
    "add".to_string()
}

async fn copy_to_library_handle(state: AppState, target_library_id: String, source_library_id: Option<String>, body: CopyToLibraryBody) -> Result<Json<Value>, ServerError> {
    let CopyToLibraryBody { book_ids, move_books, duplicate_action } = body;
    if book_ids.is_empty() {
        return Err(ServerError::BadRequest("book_ids must not be empty".to_string()));
    }
    if duplicate_action == "add_formats_to_existing" {
        // Upstream's automerge path (adding incoming formats to an
        // existing identical book) -- `calibre_db::copy_to_library`'s
        // own `copy_one_book` doesn't support it either, see that
        // module's doc.
        return Err(ServerError::BadRequest("duplicate_action=add_formats_to_existing (automerge) is not supported".to_string()));
    }
    if duplicate_action != "add" && duplicate_action != "ignore" {
        return Err(ServerError::BadRequest("duplicate_action must be one of: add, ignore".to_string()));
    }
    let check_duplicates = duplicate_action == "ignore";

    let src_cache = state
        .cache_for(source_library_id.as_deref())
        .ok_or_else(|| ServerError::NotFound(format!("no library named {:?}", source_library_id.clone().unwrap_or_default())))?;
    let dest_cache = state.cache_for(Some(&target_library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {target_library_id:?}")))?;

    let (response, copied_ids) = tokio::task::spawn_blocking(move || {
        let mut response = serde_json::Map::new();
        let mut copied_ids = Vec::new();
        for book_id in book_ids {
            match calibre_db::copy_to_library::copy_one_book(&src_cache, &dest_cache, book_id, check_duplicates) {
                Ok(Some(new_id)) => {
                    response.insert(book_id.to_string(), serde_json::json!({"ok": true, "payload": new_id}));
                    copied_ids.push(book_id);
                }
                Ok(None) => {
                    response.insert(book_id.to_string(), serde_json::json!({"ok": true, "payload": null}));
                }
                Err(e) => {
                    response.insert(book_id.to_string(), serde_json::json!({"ok": false, "payload": e.to_string()}));
                }
            }
        }
        if move_books {
            for id in &copied_ids {
                let _ = src_cache.delete_book(*id);
            }
        }
        (Value::Object(response), copied_ids)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    if !copied_ids.is_empty() {
        web_socket::publish(&state, ChangeEvent::BooksAdded { book_ids: copied_ids.clone() });
        if move_books {
            web_socket::publish(&state, ChangeEvent::BooksDeleted { book_ids: copied_ids });
        }
    }

    Ok(Json(response))
}

/// `POST /cdb/copy-to-library/{target_library_id}/{library_id}`. Port
/// of `cdb_copy_to_library`, narrowed to what
/// `calibre_db::copy_to_library::copy_one_book` supports: real
/// copy/move with same-author/near-same-title duplicate detection
/// (`duplicate_action` of `add` or `ignore`); `add_formats_to_existing`
/// (automerge) isn't ported (see that module's own doc), and neither
/// is `preserve_date`/`automerge_action` -- both accepted upstream but
/// meaningless without automerge support, so this port doesn't parse
/// them at all rather than silently ignoring accepted-but-inert
/// fields.
pub async fn copy_to_library(State(state): State<AppState>, Path((target_library_id, library_id)): Path<(String, String)>, Json(body): Json<CopyToLibraryBody>) -> Result<Json<Value>, ServerError> {
    copy_to_library_handle(state, target_library_id, Some(library_id), body).await
}

/// Same as [`copy_to_library`], for a URL with no source
/// `{library_id}` segment -- always copies from the default library.
pub async fn copy_to_library_no_source(State(state): State<AppState>, Path(target_library_id): Path<String>, Json(body): Json<CopyToLibraryBody>) -> Result<Json<Value>, ServerError> {
    copy_to_library_handle(state, target_library_id, None, body).await
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
        let state = crate::AppState { libraries: None, cache: std::sync::Arc::new(cache), opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None, changes: crate::web_socket::new_change_broadcaster(), reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()) };
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

    // `Title\n\n\nAuthor\n` is the real txt::get_metadata pattern
    // (TXT_PAT: title, three blank lines, author) -- a genuine format
    // this endpoint's metadata sniffing understands, not a stub.
    fn txt_fixture(title: &str, author: &str) -> Vec<u8> {
        format!("{title}\n\n\n{author}\n").into_bytes()
    }

    async fn post_bytes_to(router: &axum::Router, uri: &str, body: Vec<u8>) -> (StatusCode, serde_json::Value) {
        let req = Request::builder().method("POST").uri(uri).body(Body::from(body)).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value = if body.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null) };
        (status, value)
    }

    #[tokio::test]
    async fn add_book_extracts_real_metadata_and_adds_the_book() {
        let (_dir, router) = test_app(0);
        let (status, body) = post_bytes_to(&router, "/cdb/add-book/job1/n/new-book.txt/default", txt_fixture("Test Book", "Jane Doe")).await;
        assert_eq!(status, StatusCode::OK, "got: {body}");
        assert_eq!(body["title"], "Test Book");
        assert_eq!(body["authors"], serde_json::json!(["Jane Doe"]));
        assert_eq!(body["id"], "job1");
        let book_id = body["book_id"].as_i64().expect("expected a real book_id");

        let (_, fetched) = get_json(&router, &format!("/ajax/book/{book_id}")).await;
        assert_eq!(fetched["title"], "Test Book");
    }

    #[tokio::test]
    async fn add_book_rejects_a_filename_with_no_extension() {
        let (_dir, router) = test_app(0);
        let (status, _) = post_bytes_to(&router, "/cdb/add-book/job1/n/noext/default", txt_fixture("T", "A")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    /// Same as [`test_app`], but seeds one specific book (title/author)
    /// via the *same* `Cache` instance the router itself holds --
    /// unlike opening a second, separate `Cache::new` against the same
    /// directory (which briefly holds two independent
    /// `LibraryHandle`s against one library and can race: the first's
    /// OS-level write lock isn't always visibly released before the
    /// second tries to acquire it, intermittently failing with
    /// `AlreadyLocked`/"another process already holds the writer
    /// lock"). One shared `Cache` avoids the race entirely.
    fn test_app_with_book(title: &str, author: &str) -> (tempfile::TempDir, axum::Router) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        add_test_book(dir.path(), &cache, title, author);
        let state = crate::AppState { libraries: None, cache: std::sync::Arc::new(cache), opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None, changes: crate::web_socket::new_change_broadcaster(), reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()) };
        let router = crate::test_router(state);
        (dir, router)
    }

    #[tokio::test]
    async fn add_book_reports_a_duplicate_without_adding_it() {
        let (_dir, router) = test_app_with_book("Test Book", "Jane Doe");

        let (status, body) = post_bytes_to(&router, "/cdb/add-book/job1/n/new-book.txt/default", txt_fixture("Test Book", "Jane Doe")).await;
        assert_eq!(status, StatusCode::OK, "got: {body}");
        assert!(body.get("book_id").is_none(), "should not have added a duplicate, got: {body}");
        let duplicates = body["duplicates"].as_array().expect("expected a duplicates list");
        assert_eq!(duplicates.len(), 1);
        assert_eq!(duplicates[0]["title"], "Test Book");
    }

    #[tokio::test]
    async fn add_book_with_add_duplicates_flag_adds_anyway() {
        let (_dir, router) = test_app_with_book("Test Book", "Jane Doe");

        let (status, body) = post_bytes_to(&router, "/cdb/add-book/job1/y/new-book.txt/default", txt_fixture("Test Book", "Jane Doe")).await;
        assert_eq!(status, StatusCode::OK, "got: {body}");
        assert!(body.get("book_id").is_some(), "add_duplicates=y should add anyway, got: {body}");

        let (_, all_books) = get_json(&router, "/ajax/books").await;
        assert_eq!(all_books.as_object().unwrap().len(), 2);
    }

    /// Two real, separately opened libraries wired into `AppState`
    /// via a real `LibraryBroker` (issue #423), the source seeded
    /// with one book -- for exercising `copy-to-library` end to end.
    /// Returns the broker itself (not just the router) so tests can
    /// inspect/seed either library through the same already-open
    /// `Cache` the router holds, rather than opening a second,
    /// independent `Cache::new` against a directory the broker (and
    /// so the still-alive router) already has open -- see
    /// `test_app_with_book`'s own doc comment above for why that
    /// race matters.
    fn test_app_with_two_libraries() -> (tempfile::TempDir, tempfile::TempDir, std::sync::Arc<crate::library_broker::LibraryBroker>, axum::Router) {
        let src_dir = tempfile::tempdir().unwrap();
        let dest_dir = tempfile::tempdir().unwrap();
        {
            let cache = Cache::new(src_dir.path()).unwrap();
            add_test_book(src_dir.path(), &cache, "Source Book", "Jane Doe");
        }
        {
            // Seeds the dest library so it has a real metadata.db of
            // its own -- `LibraryBroker::new` only opens libraries
            // that already exist. Closed again before the broker
            // opens its own handle on the same path.
            Cache::new(dest_dir.path()).unwrap();
        }
        let broker = std::sync::Arc::new(crate::library_broker::LibraryBroker::new(&[src_dir.path().to_path_buf(), dest_dir.path().to_path_buf()]).unwrap());
        let default_cache = broker.get(None).expect("the broker's default library");
        let state = crate::AppState {
            libraries: Some(broker.clone()),
            cache: default_cache,
            opts: std::sync::Arc::new(crate::opts::ServerOptions::default()),
            auth: None,
            changes: crate::web_socket::new_change_broadcaster(),
            reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()),
        };
        let router = crate::test_router(state);
        (src_dir, dest_dir, broker, router)
    }

    #[tokio::test]
    async fn copy_to_library_copies_a_book_into_the_target_library() {
        let (src_dir, dest_dir, broker, router) = test_app_with_two_libraries();
        let src_name = src_dir.path().file_name().unwrap().to_str().unwrap();
        let dest_name = dest_dir.path().file_name().unwrap().to_str().unwrap();

        let (status, body) = post_json(&router, &format!("/cdb/copy-to-library/{dest_name}/{src_name}"), serde_json::json!({"book_ids": [1]})).await;
        assert_eq!(status, StatusCode::OK, "got: {body}");
        assert_eq!(body["1"]["ok"], true);
        assert!(body["1"]["payload"].as_i64().is_some(), "expected the new book's id, got: {body}");

        assert_eq!(broker.get(Some(dest_name)).unwrap().all_book_ids().unwrap().len(), 1, "the book should now exist in the destination library");
        assert_eq!(broker.get(Some(src_name)).unwrap().all_book_ids().unwrap().len(), 1, "a plain copy should leave the source library untouched");
    }

    #[tokio::test]
    async fn copy_to_library_with_move_removes_the_book_from_the_source() {
        let (src_dir, dest_dir, broker, router) = test_app_with_two_libraries();
        let src_name = src_dir.path().file_name().unwrap().to_str().unwrap();
        let dest_name = dest_dir.path().file_name().unwrap().to_str().unwrap();

        let (status, body) = post_json(&router, &format!("/cdb/copy-to-library/{dest_name}/{src_name}"), serde_json::json!({"book_ids": [1], "move": true})).await;
        assert_eq!(status, StatusCode::OK, "got: {body}");
        assert_eq!(body["1"]["ok"], true);

        assert!(broker.get(Some(src_name)).unwrap().all_book_ids().unwrap().is_empty(), "move should remove the book from the source library");
        assert_eq!(broker.get(Some(dest_name)).unwrap().all_book_ids().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn copy_to_library_with_ignore_skips_an_existing_duplicate() {
        let (src_dir, dest_dir, broker, router) = test_app_with_two_libraries();
        let src_name = src_dir.path().file_name().unwrap().to_str().unwrap();
        let dest_name = dest_dir.path().file_name().unwrap().to_str().unwrap();
        add_test_book(dest_dir.path(), &broker.get(Some(dest_name)).unwrap(), "Source Book", "Jane Doe");

        let (status, body) = post_json(&router, &format!("/cdb/copy-to-library/{dest_name}/{src_name}"), serde_json::json!({"book_ids": [1], "duplicate_action": "ignore"})).await;
        assert_eq!(status, StatusCode::OK, "got: {body}");
        assert_eq!(body["1"]["ok"], true);
        assert!(body["1"]["payload"].is_null(), "a detected duplicate should report a null payload, got: {body}");

        assert_eq!(broker.get(Some(dest_name)).unwrap().all_book_ids().unwrap().len(), 1, "the duplicate should not have been added");
    }

    #[tokio::test]
    async fn copy_to_library_404s_for_an_unknown_target_library() {
        let (src_dir, _dest_dir, _broker, router) = test_app_with_two_libraries();
        let src_name = src_dir.path().file_name().unwrap().to_str().unwrap();

        let (status, _) = post_json(&router, &format!("/cdb/copy-to-library/NoSuchLibrary/{src_name}"), serde_json::json!({"book_ids": [1]})).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn copy_to_library_rejects_automerge_duplicate_action() {
        let (src_dir, dest_dir, _broker, router) = test_app_with_two_libraries();
        let src_name = src_dir.path().file_name().unwrap().to_str().unwrap();
        let dest_name = dest_dir.path().file_name().unwrap().to_str().unwrap();

        let (status, _) = post_json(&router, &format!("/cdb/copy-to-library/{dest_name}/{src_name}"), serde_json::json!({"book_ids": [1], "duplicate_action": "add_formats_to_existing"})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}

