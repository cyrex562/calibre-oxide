//! Port of `books.py`'s `mathjax`/`manifest_as_json`/
//! `get_mathjax_manifest` (issue #484): serves the vendored MathJax
//! bundle books with embedded math need in the browser reader.
//!
//! # The vendored bundle
//!
//! `resources/mathjax/` (issue #484, following `old_src/setup/mathjax.py`'s
//! own real file selection) is real MathJax 3.1.4's ES5 build --
//! `core.js`/`loader.js`/`startup.js`, the `tex-full`/`asciimath`/`mml`
//! input processors, the `chtml` output processor, and its font tree
//! (32 files, ~1.4MB, Apache-2.0 licensed) -- embedded into the
//! compiled binary at build time via [`include_dir`], the same
//! bake-resources-into-the-binary convention `calibre_db`'s
//! `metadata_sqlite.sql` already uses (`include_str!`), extended here
//! to a whole directory tree since a browser reader needs more than
//! one file. `manifest.json` (also vendored, not generated at
//! runtime) records each file's byte size and one overall `etag` --
//! this port computed that etag as a real SHA-1 over every vendored
//! file's bytes in sorted-path order, so it's a real, verifiable
//! digest of what's actually in the bundle; it is not expected to
//! match any specific historical value from a real calibre release
//! build (upstream never checks its own generated manifest into git
//! either -- it's produced fresh by each build's own file-system
//! iteration order, which was never a promised stable value to begin
//! with).
//!
//! # Disclosed narrowing
//!
//! **No conditional-GET (`If-None-Match` -> `304`).** A real `ETag`
//! header is set on every response, so a client that understands
//! `ETag` still avoids re-downloading unchanged bytes on its own
//! terms, but this crate doesn't inspect `If-None-Match` and reply
//! `304` itself -- the same disclosed simplification `content.rs`
//! already uses for cover/format downloads, not something new.
//!
//! # Not behind the auth middleware
//!
//! Upstream's `@endpoint('/mathjax/{+which=""}', auth_required=False)`
//! is deliberate: a reader page can reference these assets from
//! contexts (cached pages, embedded iframes) where re-authenticating
//! isn't practical, and the bundle has no per-library or per-user
//! content in it to protect. `router()` merges this module's routes
//! in *after* the rest of the API gets its `auth::require_auth`
//! `route_layer`, so they're the one real exemption in this crate --
//! see `router()`'s own doc for why nothing else needed one yet.

use axum::extract::Path;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use include_dir::{include_dir, Dir};
use serde_json::Value;
use std::sync::OnceLock;

static MATHJAX_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/resources/mathjax");
static MANIFEST: OnceLock<Value> = OnceLock::new();

fn manifest() -> &'static Value {
    MANIFEST.get_or_init(|| {
        let file = MATHJAX_DIR.get_file("manifest.json").expect("manifest.json is vendored alongside the bundle");
        serde_json::from_slice(file.contents()).expect("vendored manifest.json is always valid JSON")
    })
}

fn manifest_etag(manifest: &Value) -> String {
    format!("\"{}\"", manifest["etag"].as_str().unwrap_or(""))
}

fn manifest_response() -> Response {
    let manifest = manifest();
    let body = serde_json::to_vec(manifest).expect("a loaded JSON Value always re-serializes");
    let mut resp = body.into_response();
    resp.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json; charset=UTF-8"));
    if let Ok(v) = HeaderValue::from_str(&manifest_etag(manifest)) {
        resp.headers_mut().insert(header::ETAG, v);
    }
    resp
}

fn not_found(which: &str) -> Response {
    (StatusCode::NOT_FOUND, format!("No MathJax file named: {which}")).into_response()
}

/// `GET /mathjax`. Port of `mathjax` with no `which`: the manifest
/// itself.
pub async fn mathjax_root() -> Response {
    manifest_response()
}

/// `GET /mathjax/{*which}`. Port of `mathjax` with a `which`: one
/// vendored file. `which` must be an exact key in the manifest's
/// `files` map -- this doubles as upstream's path-traversal guard
/// (its own `abspath`-prefix check), since [`include_dir::Dir::get_file`]
/// only ever resolves paths that exist inside the tree embedded at
/// compile time in the first place; nothing external for a `..` to
/// escape to.
pub async fn mathjax_file(Path(which): Path<String>) -> Response {
    if which.is_empty() {
        return manifest_response();
    }
    let manifest = manifest();
    let known = manifest.get("files").and_then(Value::as_object).map(|files| files.contains_key(&which)).unwrap_or(false);
    if !known {
        return not_found(&which);
    }
    let Some(file) = MATHJAX_DIR.get_file(which.as_str()) else {
        return not_found(&which);
    };

    let mime = mime_guess::from_path(&which).first_raw().unwrap_or("application/octet-stream");
    let mut resp = file.contents().to_vec().into_response();
    if let Ok(v) = HeaderValue::from_str(mime) {
        resp.headers_mut().insert(header::CONTENT_TYPE, v);
    }
    if let Ok(v) = HeaderValue::from_str(&manifest_etag(manifest)) {
        resp.headers_mut().insert(header::ETAG, v);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::Request;
    use tower::ServiceExt;

    fn test_router() -> axum::Router {
        axum::Router::new().route("/mathjax", axum::routing::get(mathjax_root)).route("/mathjax/{*which}", axum::routing::get(mathjax_file))
    }

    #[tokio::test]
    async fn root_returns_the_manifest_with_a_real_etag() {
        let router = test_router();
        let resp = router.oneshot(Request::builder().uri("/mathjax").body(axum::body::Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get(header::CONTENT_TYPE).unwrap(), "application/json; charset=UTF-8");
        let etag = resp.headers().get(header::ETAG).unwrap().to_str().unwrap().to_string();
        assert!(etag.len() > 10, "{etag}");
        let body: Value = serde_json::from_slice(&to_bytes(resp.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(body["version"], "3.1.4");
        assert!(body["files"].as_object().unwrap().contains_key("core.js"));
    }

    #[tokio::test]
    async fn a_known_file_is_served_with_the_manifests_etag_and_a_real_mime_type() {
        let router = test_router();
        let resp = router.oneshot(Request::builder().uri("/mathjax/core.js").body(axum::body::Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get(header::CONTENT_TYPE).unwrap(), "text/javascript");
        let etag = resp.headers().get(header::ETAG).unwrap().to_str().unwrap().to_string();
        assert_eq!(etag, manifest_etag(manifest()));
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert!(!body.is_empty());
    }

    #[tokio::test]
    async fn a_nested_font_file_is_served_correctly() {
        let router = test_router();
        let resp = router.oneshot(Request::builder().uri("/mathjax/output/chtml/fonts/tex.js").body(axum::body::Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn an_unlisted_path_404s() {
        let router = test_router();
        let resp = router.oneshot(Request::builder().uri("/mathjax/not-a-real-file.js").body(axum::body::Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_path_traversal_attempt_is_rejected_not_served() {
        let router = test_router();
        // Not in the manifest's `files` map regardless of how it's
        // spelled, so the same not-in-manifest check that rejects any
        // other unknown path also rejects this.
        let resp = router.oneshot(Request::builder().uri("/mathjax/../Cargo.toml").body(axum::body::Body::empty()).unwrap()).await.unwrap();
        assert_ne!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn every_manifest_listed_file_is_actually_servable() {
        let manifest = manifest();
        let files = manifest["files"].as_object().unwrap();
        for path in files.keys() {
            let router = test_router();
            let resp = router.oneshot(Request::builder().uri(format!("/mathjax/{path}")).body(axum::body::Body::empty()).unwrap()).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "failed to serve {path}");
        }
    }
}
