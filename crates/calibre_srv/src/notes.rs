//! Port of a subset of `calibre.srv.content` -- the "Notes" feature
//! (free-form HTML notes attachable to a category item, e.g. an
//! author or tag, with embedded image resources), backed by
//! `calibre_db`'s already-ported notes engine (issue #227's
//! `Cache::notes`/`NotesConnection`).
//!
//! # Scope
//!
//! `field` is restricted to the same five standard categories
//! `calibre_db::categories` supports (`authors`/`tags`/`series`/
//! `publisher`/`languages`) -- upstream supports notes on any field
//! with `supports_notes` metadata (including custom columns), which
//! needs the full `field_metadata` system this crate doesn't have
//! (same disclosed narrowing as every other category-touching module
//! in this crate). `library_id` is a required path segment here
//! (accepted but unused, single-library-only, same as everywhere else
//! in this crate), not upstream's optional trailing segment -- no
//! `{path}`-without-`library_id` route variant is registered.
//!
//! - `GET /get-note/{field}/{item_id}/{library_id}` -- the note's
//!   HTML, with `calres://scheme/digest` resource placeholders
//!   rewritten to real `/get-note-resource/...` URLs.
//! - `GET /get-note-from-item-val/{field}/{item}/{library_id}` --
//!   same, but resolves `item` (a display name, e.g. an author's
//!   name) to an id via `calibre_db::categories::get_item_id` first.
//! - `GET /get-note-resource/{scheme}/{digest}/{library_id}` -- the
//!   raw bytes of one embedded resource (e.g. an image). `scheme`/
//!   `digest` are validated as alphanumeric-only before being used to
//!   build a lookup key -- see `NotesConnection::path_for_resource`'s
//!   own doc for why (this endpoint is exactly the reason that
//!   function needed hardening against a `../`-laden hash).
//! - `POST /set-note/{field}/{item_id}/{library_id}` -- `{html,
//!   searchable_text, images: {key: {data, filename}}}`; each image
//!   is either a new `data:` URL (stored via `add_resource`) or a
//!   reference to an existing `/get-note-resource/{scheme}/{digest}`
//!   URL (an already-attached image being kept). `{key}` tokens in
//!   `html` are replaced with the real resource URL for the response
//!   and a `calres://` placeholder for storage, matching upstream.
//!   `searchable_text` is accepted but not used --
//!   `NotesConnection::set_note` computes its own from the item's
//!   display name and the HTML, with no override parameter.

use std::collections::{HashMap, HashSet};

use axum::extract::{Path, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine;
use serde::Deserialize;

use calibre_db::cache::Cache;
use calibre_db::categories;
use calibre_db::constants::RESOURCE_URL_SCHEME;

use crate::errors::ServerError;
use crate::AppState;

fn resource_hash_to_url(scheme: &str, digest: &str) -> String {
    format!("/get-note-resource/{scheme}/{digest}")
}

/// Rewrites every `calres://scheme/digest` placeholder in `html`
/// (restricted to the hashes actually attached to this note, matching
/// upstream's own precomputed-pattern approach) to a real
/// `/get-note-resource/...` URL.
fn rewrite_resource_urls(html: &str, resource_hashes: &HashSet<String>) -> String {
    let mut out = html.to_string();
    for rhash in resource_hashes {
        let Some((scheme, digest)) = rhash.split_once(':') else { continue };
        let placeholder = format!("{RESOURCE_URL_SCHEME}://{scheme}/{digest}");
        out = out.replace(&placeholder, &resource_hash_to_url(scheme, digest));
    }
    out
}

/// Port of `_get_note`. `Ok(None)` means the item itself doesn't
/// exist (caller should 404); `Ok(Some(""))` means the item exists
/// but has no note yet (matches upstream returning `''`).
fn get_note_html(cache: &Cache, field: &str, item_id: i32) -> anyhow::Result<Option<String>> {
    cache.notes().initialize()?;
    match cache.notes().get_note_data(field, item_id)? {
        Some(data) => Ok(Some(rewrite_resource_urls(&data.doc, &data.resource_hashes))),
        None => {
            if categories::get_item_name(cache, field, item_id)?.is_some() {
                Ok(Some(String::new()))
            } else {
                Ok(None)
            }
        }
    }
}

fn is_alphanumeric(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric())
}

/// `GET /get-note/{field}/{item_id}/{library_id}`. Port of `get_note`.
pub async fn get_note(State(state): State<AppState>, Path((field, item_id, _library_id)): Path<(String, i32, String)>) -> Result<Response, ServerError> {
    let html = tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        let field = field.clone();
        move || get_note_html(&cache, &field, item_id)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    let Some(html) = html else {
        return Err(ServerError::NotFound(format!("Item {field:?}:{item_id} not found")));
    };

    let mut resp = html.into_response();
    resp.headers_mut().insert(header::CONTENT_TYPE, header::HeaderValue::from_static("text/html; charset=UTF-8"));
    Ok(resp)
}

/// `GET /get-note-from-item-val/{field}/{item}/{library_id}`. Port of
/// `get_note_from_val`.
pub async fn get_note_from_val(State(state): State<AppState>, Path((field, item, _library_id)): Path<(String, String, String)>) -> Result<Json<serde_json::Value>, ServerError> {
    let result = tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        let field = field.clone();
        move || -> anyhow::Result<Option<(i32, Option<String>)>> {
            let Some(item_id) = categories::get_item_id(&cache, &field, &item)? else {
                return Ok(None);
            };
            Ok(Some((item_id, get_note_html(&cache, &field, item_id)?)))
        }
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    let Some((item_id, Some(html))) = result else {
        return Err(ServerError::NotFound(format!("Item {field:?} not found")));
    };

    Ok(Json(serde_json::json!({ "item_id": item_id, "html": html })))
}

/// `GET /get-note-resource/{scheme}/{digest}/{library_id}`. Port of
/// `get_note_resource`.
pub async fn get_note_resource(State(state): State<AppState>, Path((scheme, digest, _library_id)): Path<(String, String, String)>) -> Result<Response, ServerError> {
    if !is_alphanumeric(&scheme) || !is_alphanumeric(&digest) {
        return Err(ServerError::NotFound(format!("Notes resource {scheme}:{digest} not found")));
    }
    let hash = format!("{scheme}:{digest}");

    let resource = tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        move || -> anyhow::Result<Option<calibre_db::notes::connection::ResourceData>> {
            cache.notes().initialize()?;
            Ok(cache.notes().get_resource_data(&hash)?)
        }
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    let Some(resource) = resource else {
        return Err(ServerError::NotFound(format!("Notes resource {scheme}:{digest} not found")));
    };

    // These resources are always client-uploaded (`set_note`'s
    // `images`) and served back with no auth distinction from the
    // uploader -- serving an attacker-chosen filename `inline` with a
    // guessed Content-Type (upstream's own behavior too, `guess_type
    // (name)[0]`) is a real stored-XSS vector: an uploaded
    // `evil.svg`/`evil.html` resource would render as same-origin
    // markup/script if navigated to directly, able to replay a
    // browser's cached HTTP Basic credentials against every other
    // endpoint. Narrowed here, beyond upstream's own fidelity, to a
    // real image-only allowlist -- anything else downloads as an
    // attachment with a generic type instead of rendering inline.
    let content_type = match mime_guess::from_path(&resource.name).first_raw() {
        Some(m @ ("image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/bmp" | "image/avif")) => m,
        _ => "application/octet-stream",
    };
    let disposition = if content_type == "application/octet-stream" { "attachment" } else { "inline" };

    let mut resp = resource.data.into_response();
    resp.headers_mut().insert(header::CONTENT_TYPE, header::HeaderValue::from_static(content_type));
    if let Ok(v) = header::HeaderValue::from_str(&format!("{disposition}; filename=\"{}\"", resource.name)) {
        resp.headers_mut().insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(resp)
}

#[derive(Debug, Deserialize)]
struct ImageSpec {
    data: String,
    #[serde(default)]
    filename: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SetNoteBody {
    html: String,
    #[serde(default)]
    searchable_text: String,
    #[serde(default)]
    images: HashMap<String, ImageSpec>,
}

/// Extracts `scheme`/`digest` from an existing `/get-note-resource/
/// {scheme}/{digest}` URL embedded in `s` (an image reference to a
/// resource already attached to some note, being kept as-is), the
/// same way upstream's `res_pat` regex does.
fn extract_existing_resource_ref(s: &str) -> Option<(String, String)> {
    let rest = s.split("get-note-resource/").nth(1)?;
    let mut parts = rest.splitn(3, '/');
    let scheme = parts.next()?;
    let digest = parts.next()?;
    let digest = digest.split(|c: char| !c.is_ascii_alphanumeric()).next().unwrap_or(digest);
    if is_alphanumeric(scheme) && is_alphanumeric(digest) {
        Some((scheme.to_string(), digest.to_string()))
    } else {
        None
    }
}

fn decode_data_url(data: &str) -> Result<Vec<u8>, ServerError> {
    let rest = data.strip_prefix("data:").ok_or_else(|| ServerError::BadRequest("Invalid query: not a data: URL".to_string()))?;
    let (_, payload) = rest.split_once(',').ok_or_else(|| ServerError::BadRequest("Invalid query: malformed data: URL".to_string()))?;
    base64::engine::general_purpose::STANDARD.decode(payload).map_err(|e| ServerError::BadRequest(format!("Invalid query: {e}")))
}

/// `POST /set-note/{field}/{item_id}/{library_id}`. Port of `set_note`.
pub async fn set_note(State(state): State<AppState>, Path((field, item_id, _library_id)): Path<(String, i32, String)>, Json(body): Json<SetNoteBody>) -> Result<Response, ServerError> {
    let mut db_replacements: HashMap<String, String> = HashMap::new();
    let mut srv_replacements: HashMap<String, String> = HashMap::new();
    let mut resources: HashSet<String> = HashSet::new();

    for (key, img) in &body.images {
        let (scheme, digest) = if img.data.starts_with("data:") {
            let bytes = decode_data_url(&img.data)?;
            let filename = img.filename.clone().ok_or_else(|| ServerError::BadRequest("Invalid query: image has no filename".to_string()))?;
            let cache = state.cache.clone();
            let bytes_clone = bytes.clone();
            let filename_clone = filename.clone();
            let rhash = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
                cache.notes().initialize()?;
                Ok(cache.notes().add_resource(&bytes_clone, &filename_clone)?)
            })
            .await
            .map_err(|e| ServerError::InternalServerError(e.to_string()))?
            .map_err(|e| ServerError::InternalServerError(e.to_string()))?;
            let (s, d) = rhash.split_once(':').unwrap_or(("raw", rhash.as_str()));
            (s.to_string(), d.to_string())
        } else {
            extract_existing_resource_ref(&img.data).ok_or_else(|| ServerError::BadRequest(format!("Invalid query: unrecognized image reference for {key:?}")))?
        };
        resources.insert(format!("{scheme}:{digest}"));
        srv_replacements.insert(key.clone(), resource_hash_to_url(&scheme, &digest));
        db_replacements.insert(key.clone(), format!("{RESOURCE_URL_SCHEME}://{scheme}/{digest}"));
    }

    let mut db_html = body.html.clone();
    let mut srv_html = body.html.clone();
    for (key, val) in &db_replacements {
        db_html = db_html.replace(key.as_str(), val);
    }
    for (key, val) in &srv_replacements {
        srv_html = srv_html.replace(key.as_str(), val);
    }

    tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        let field = field.clone();
        move || -> Result<(), ServerError> {
            let item_value = categories::get_item_name(&cache, &field, item_id).map_err(|e| ServerError::InternalServerError(e.to_string()))?.ok_or_else(|| ServerError::NotFound(format!("Item {field:?}:{item_id} not found")))?;
            cache.notes().initialize().map_err(|e| ServerError::InternalServerError(e.to_string()))?;
            cache.notes().set_note(&field, item_id, &item_value, &db_html, &resources).map_err(|e| ServerError::InternalServerError(e.to_string()))?;
            Ok(())
        }
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))??;

    // `body.searchable_text` is accepted for request-shape compatibility
    // but not used: `NotesConnection::set_note` computes its own
    // searchable text internally (`item_value + "\n" + marked_up_text`,
    // see that function's own doc) and has no parameter for a
    // caller-supplied override -- a real, disclosed narrowing, not a
    // silently dropped field.
    let _ = body.searchable_text;

    let mut resp = srv_html.into_response();
    resp.headers_mut().insert(header::CONTENT_TYPE, header::HeaderValue::from_static("text/html; charset=UTF-8"));
    Ok(resp)
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
        cache.notes().initialize().unwrap();
        let state = crate::AppState { libraries: None, cache: cache.clone(), opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None, changes: crate::web_socket::new_change_broadcaster(), reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()), book_cache: std::sync::Arc::new(crate::books_cache::BookCache::open_temp()), jobs: std::sync::Arc::new(crate::jobs::JobsManager::new(4, std::time::Duration::from_secs(3600))), render_jobs: std::sync::Arc::new(crate::render_endpoints::RenderJobRegistry::new()) };
        let router = crate::test_router(state);
        (dir, router, cache)
    }

    async fn get_body(router: &axum::Router, uri: &str) -> (StatusCode, String) {
        let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    async fn post_json(router: &axum::Router, uri: &str, body: serde_json::Value) -> (StatusCode, String) {
        let req = Request::builder().method("POST").uri(uri).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    #[tokio::test]
    async fn get_note_404s_for_an_unknown_item() {
        let (_dir, router, _cache) = test_app();
        let (status, _) = get_body(&router, "/get-note/authors/999/default").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_note_returns_empty_string_for_an_item_with_no_note_yet() {
        let (dir, router, cache) = test_app();
        add_test_book(dir.path(), &cache, "Book A", "Jane Doe");
        let author_id = calibre_db::categories::get_item_id(&cache, "authors", "Jane Doe").unwrap().unwrap();

        let (status, body) = get_body(&router, &format!("/get-note/authors/{author_id}/default")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "");
    }

    #[tokio::test]
    async fn get_note_rewrites_resource_urls_in_stored_html() {
        let (dir, router, cache) = test_app();
        add_test_book(dir.path(), &cache, "Book A", "Jane Doe");
        let author_id = calibre_db::categories::get_item_id(&cache, "authors", "Jane Doe").unwrap().unwrap();

        let rhash = cache.notes().add_resource(b"fake image bytes", "photo.jpg").unwrap();
        let (scheme, digest) = rhash.split_once(':').unwrap();
        let doc = format!("<p>Bio</p><img src=\"calres://{scheme}/{digest}\">");
        cache.notes().set_note("authors", author_id, "Jane Doe", &doc, &[rhash.clone()].into_iter().collect()).unwrap();

        let (status, body) = get_body(&router, &format!("/get-note/authors/{author_id}/default")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, format!("<p>Bio</p><img src=\"/get-note-resource/{scheme}/{digest}\">"));
    }

    #[tokio::test]
    async fn get_note_from_val_resolves_by_display_name() {
        let (dir, router, cache) = test_app();
        add_test_book(dir.path(), &cache, "Book A", "Jane Doe");
        let author_id = calibre_db::categories::get_item_id(&cache, "authors", "Jane Doe").unwrap().unwrap();
        cache.notes().set_note("authors", author_id, "Jane Doe", "<p>Bio</p>", &Default::default()).unwrap();

        let (status, body) = get_body(&router, "/get-note-from-item-val/authors/Jane%20Doe/default").await;
        assert_eq!(status, StatusCode::OK);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["item_id"], author_id);
        assert_eq!(v["html"], "<p>Bio</p>");
    }

    #[tokio::test]
    async fn get_note_from_val_404s_for_an_unknown_name() {
        let (_dir, router, _cache) = test_app();
        let (status, _) = get_body(&router, "/get-note-from-item-val/authors/Nobody/default").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_note_resource_serves_real_bytes_with_content_disposition() {
        let (_dir, router, cache) = test_app();
        let rhash = cache.notes().add_resource(b"jpeg bytes here", "photo.jpg").unwrap();
        let (scheme, digest) = rhash.split_once(':').unwrap();

        let req = Request::builder().uri(format!("/get-note-resource/{scheme}/{digest}/default")).body(Body::empty()).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get("content-type").unwrap(), "image/jpeg");
        assert!(resp.headers().get("content-disposition").unwrap().to_str().unwrap().contains("photo.jpg"));
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"jpeg bytes here");
    }

    #[tokio::test]
    async fn get_note_resource_serves_a_non_image_as_an_octet_stream_attachment_not_inline() {
        // Regression test for a stored-XSS finding: an uploaded
        // "evil.html"/"evil.svg" resource must not be served inline
        // with a browser-executable Content-Type, even though the
        // filename (and thus the guessed MIME type) is fully
        // attacker-controlled at upload time.
        let (_dir, router, cache) = test_app();
        let rhash = cache.notes().add_resource(b"<script>alert(1)</script>", "evil.html").unwrap();
        let (scheme, digest) = rhash.split_once(':').unwrap();

        let req = Request::builder().uri(format!("/get-note-resource/{scheme}/{digest}/default")).body(Body::empty()).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get("content-type").unwrap(), "application/octet-stream");
        assert!(resp.headers().get("content-disposition").unwrap().to_str().unwrap().starts_with("attachment"));
    }

    #[tokio::test]
    async fn get_note_resource_404s_for_an_unknown_hash() {
        let (_dir, router, _cache) = test_app();
        let (status, _) = get_body(&router, "/get-note-resource/siphash64/deadbeef/default").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_note_resource_rejects_a_path_traversal_digest() {
        let (_dir, router, _cache) = test_app();
        let (status, _) = get_body(&router, "/get-note-resource/siphash64/..%2f..%2f..%2fetc%2fpasswd/default").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn set_note_stores_a_new_base64_image_and_rewrites_the_key_token() {
        use base64::Engine;
        let (dir, router, cache) = test_app();
        add_test_book(dir.path(), &cache, "Book A", "Jane Doe");
        let author_id = calibre_db::categories::get_item_id(&cache, "authors", "Jane Doe").unwrap().unwrap();
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"new photo bytes");

        let (status, body) = post_json(
            &router,
            &format!("/set-note/authors/{author_id}/default"),
            serde_json::json!({
                "html": "<p>Bio</p><img src=\"{{img1}}\">",
                "searchable_text": "Bio",
                "images": {"{{img1}}": {"data": format!("data:image/jpeg;base64,{encoded}"), "filename": "photo.jpg"}},
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "got: {body}");
        assert!(body.contains("/get-note-resource/"), "expected a real resource URL in the response, got: {body}");
        assert!(!body.contains("{{img1}}"), "expected the key token to be replaced, got: {body}");

        // The stored (DB) form uses a calres:// placeholder, not the
        // real HTTP URL -- confirmed by reading it straight back via
        // get-note and checking it now has the SAME rewritten form
        // (proving round-trip storage worked, not just the immediate
        // response).
        let (_, refetched) = get_body(&router, &format!("/get-note/authors/{author_id}/default")).await;
        assert!(refetched.contains("/get-note-resource/"));
    }

    #[tokio::test]
    async fn set_note_reuses_an_existing_resource_reference() {
        let (dir, router, cache) = test_app();
        add_test_book(dir.path(), &cache, "Book A", "Jane Doe");
        let author_id = calibre_db::categories::get_item_id(&cache, "authors", "Jane Doe").unwrap().unwrap();
        let rhash = cache.notes().add_resource(b"existing photo", "old.jpg").unwrap();
        let (scheme, digest) = rhash.split_once(':').unwrap();

        let (status, body) = post_json(
            &router,
            &format!("/set-note/authors/{author_id}/default"),
            serde_json::json!({
                "html": "<p>Bio</p><img src=\"{{img1}}\">",
                "searchable_text": "Bio",
                "images": {"{{img1}}": {"data": format!("/get-note-resource/{scheme}/{digest}")}},
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "got: {body}");
        assert!(body.contains(&format!("/get-note-resource/{scheme}/{digest}")));
    }

    #[tokio::test]
    async fn set_note_404s_for_an_unknown_item() {
        let (_dir, router, _cache) = test_app();
        let (status, _) = post_json(&router, "/set-note/authors/999/default", serde_json::json!({"html": "<p>x</p>", "searchable_text": "x", "images": {}})).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
