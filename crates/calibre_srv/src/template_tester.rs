//! `POST /template-tester/evaluate/{book_id}/{library_id}` -- a real,
//! **new** route (issue #763), not a port (real upstream's Template
//! Tester dialog is Qt GUI-only, never exposed over HTTP). Evaluates a
//! real calibre-template-language string against one real book's
//! field data, using `calibre_utils::formatter`'s already-real lexer/
//! parser/evaluator (#513-524 epic) and `calibre_db::formatter_functions`'s
//! already-real `Cache`-backed `ValueSource`/`FunctionRegistry`/
//! `FunctionCatalog` implementations (#514/#515/#518/#524) -- both
//! fully ported and tested, but confirmed via grep to have had **zero
//! real callers anywhere in this workspace** before this route: the
//! same "real backend, zero HTTP exposure" gap this whole issue
//! cluster keeps finding.
//!
//! # Real, disclosed narrowing: Template Program Mode only, no `{field}` shorthand
//!
//! Real upstream calibre templates come in two dialects (confirmed by
//! reading `TemplateFormatter.evaluate`'s real dispatch, already
//! ported once for `calibre_ebooks::covers`'s own narrower use case):
//! a `program:`-prefixed full "Template Program Mode" (GPM) body, e.g.
//! `program: return field('title')`, and a default `{field}`/
//! `{field:format_spec}` `string.Formatter`-style substitution
//! dialect (`covers.rs`'s own private `vformat`). This route accepts
//! GPM only -- a bare `field('title')` (an optional leading
//! `program:` is stripped if present, matching upstream's own real
//! dispatch) -- and does **not** accept the `{field}` shorthand.
//! `vformat` isn't reused here because it's written concretely against
//! `covers.rs`'s own private `CoverValueSource`, not a generic
//! `ValueSource`; generalizing it is real, separable follow-up work if
//! the shorthand dialect is ever needed through this route.

use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use calibre_db::formatter_functions::{CacheCatalog, CacheFunctions, CacheValueSource};
use calibre_utils::formatter::{interp, lexer, parser};

use crate::errors::ServerError;
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct EvaluateBody {
    template: String,
}

/// `POST /template-tester/evaluate/{book_id}/{library_id}`.
pub async fn evaluate(State(state): State<AppState>, Path((book_id, library_id)): Path<(i32, String)>, Json(body): Json<EvaluateBody>) -> Result<Json<Value>, ServerError> {
    let cache = state.cache_for(Some(&library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))?;

    let result = tokio::task::spawn_blocking(move || -> Result<String, String> {
        let program_text = body.template.strip_prefix("program:").unwrap_or(&body.template);
        let tokens = lexer::scan(program_text).map_err(|pos| format!("Formatter: unexpected character at position {pos}"))?;
        let program = parser::parse(&tokens, &CacheCatalog, Default::default()).map_err(|e| e.to_string())?;
        let value_source = CacheValueSource::new(&cache, book_id).map_err(|e| e.to_string())?;
        let functions = CacheFunctions::new(&cache, book_id);
        let mut globals = std::collections::HashMap::new();
        interp::evaluate(&program, "", Box::new(value_source), &functions, &mut globals).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    match result {
        Ok(output) => Ok(Json(json!({"ok": true, "result": output}))),
        // A bad template (syntax error, unknown function, an unknown
        // book id) is a real, expected outcome a template tester needs
        // to show the user, not a 500 -- matches this route's own
        // real purpose (the user is actively iterating on a template
        // that may well be broken).
        Err(error) => Ok(Json(json!({"ok": false, "error": error}))),
    }
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use calibre_db::cache::Cache;

    fn test_app() -> (tempfile::TempDir, axum::Router, i32) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let source = dir.path().join("Book.txt");
        std::fs::write(&source, b"hello").unwrap();
        let mut meta = calibre_ebooks::metadata::MetaInformation::default();
        meta.title = "My Title".to_string();
        meta.authors = vec!["Jane Doe".to_string()];
        let book_id = cache.add_book(&source, &meta).unwrap();
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
        };
        let router = crate::test_router(state);
        (dir, router, book_id)
    }

    async fn post_json(router: &axum::Router, uri: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let req = Request::builder().method("POST").uri(uri).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value = if bytes.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null) };
        (status, value)
    }

    #[tokio::test]
    async fn evaluates_a_real_field_reference_against_a_real_book() {
        let (_dir, router, book_id) = test_app();
        let (status, body) = post_json(&router, &format!("/template-tester/evaluate/{book_id}/default"), serde_json::json!({"template": "field('title')"})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["ok"], true);
        assert_eq!(body["result"], "My Title");
    }

    #[tokio::test]
    async fn evaluates_a_real_template_function() {
        let (_dir, router, book_id) = test_app();
        let (status, body) = post_json(&router, &format!("/template-tester/evaluate/{book_id}/default"), serde_json::json!({"template": "uppercase(field('title'))"})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["ok"], true);
        assert_eq!(body["result"], "MY TITLE");
    }

    #[tokio::test]
    async fn a_leading_program_prefix_is_stripped_matching_real_upstream_syntax() {
        let (_dir, router, book_id) = test_app();
        let (status, body) = post_json(&router, &format!("/template-tester/evaluate/{book_id}/default"), serde_json::json!({"template": "program: field('title')"})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["ok"], true);
        assert_eq!(body["result"], "My Title");
    }

    #[tokio::test]
    async fn reports_a_real_syntax_error_as_ok_false_not_a_500() {
        let (_dir, router, book_id) = test_app();
        let (status, body) = post_json(&router, &format!("/template-tester/evaluate/{book_id}/default"), serde_json::json!({"template": "field('title'"})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["ok"], false);
        assert!(body["error"].as_str().unwrap().len() > 0);
    }

    #[tokio::test]
    async fn reports_an_unknown_function_as_ok_false() {
        let (_dir, router, book_id) = test_app();
        let (status, body) = post_json(&router, &format!("/template-tester/evaluate/{book_id}/default"), serde_json::json!({"template": "not_a_real_function(field('title'))"})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["ok"], false);
    }

    #[tokio::test]
    async fn a_real_custom_column_reference_evaluates_correctly() {
        let (_dir, router, book_id) = test_app();
        let (status, _) = post_json(&router, "/custom-columns/add", serde_json::json!({"label": "shelf", "name": "Shelf", "datatype": "text"})).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = post_json(&router, &format!("/cdb/set-fields/{book_id}"), serde_json::json!({"changes": {"shelf": "Living Room"}})).await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = post_json(&router, &format!("/template-tester/evaluate/{book_id}/default"), serde_json::json!({"template": "field('#shelf')"})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["ok"], true);
        assert_eq!(body["result"], "Living Room");
    }

    fn test_app_with_two_libraries() -> (tempfile::TempDir, tempfile::TempDir, std::sync::Arc<crate::library_broker::LibraryBroker>, axum::Router, i32) {
        let src_dir = tempfile::tempdir().unwrap();
        let dest_dir = tempfile::tempdir().unwrap();
        let book_id = {
            let cache = Cache::new(src_dir.path()).unwrap();
            let source = src_dir.path().join("Book.txt");
            std::fs::write(&source, b"hello").unwrap();
            let mut meta = calibre_ebooks::metadata::MetaInformation::default();
            meta.title = "My Title".to_string();
            cache.add_book(&source, &meta).unwrap()
        };
        Cache::new(dest_dir.path()).unwrap();
        let broker = std::sync::Arc::new(crate::library_broker::LibraryBroker::new(&[src_dir.path().to_path_buf(), dest_dir.path().to_path_buf()]).unwrap());
        let default_cache = broker.get(None).expect("the broker's default library");
        let state = crate::AppState {
            libraries: Some(broker.clone()),
            cache: default_cache,
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
        };
        let router = crate::test_router(state);
        (src_dir, dest_dir, broker, router, book_id)
    }

    #[tokio::test]
    async fn evaluate_404s_for_an_unknown_library_id() {
        // Must use multi-library mode: in single-library mode
        // (`AppState::libraries == None`), `cache_for` always falls
        // back to the default library regardless of the requested
        // name -- an unknown name can only actually 404 once a real
        // `LibraryBroker` is involved (matches this project's own
        // established test pattern, e.g. cdb.rs).
        let (_src_dir, _dest_dir, _broker, router, book_id) = test_app_with_two_libraries();
        let (status, _) = post_json(&router, &format!("/template-tester/evaluate/{book_id}/no-such-library"), serde_json::json!({"template": "field('title')"})).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
