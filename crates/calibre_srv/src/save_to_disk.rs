//! `POST /save-to-disk/{library_id}` -- port of `library.save_to_disk`
//! (issue #751, part of #747's tracking epic): export one or more
//! books into a destination folder tree, naming each file/folder from
//! a user-configurable real calibre-template-language string (real
//! upstream default: `'{author_sort}/{title}/{title} - {authors}'`),
//! matching `library.save_to_disk.get_components`/`get_path_components`'s
//! real per-component `/`-split + sanitize behavior.
//!
//! Uses `calibre_utils::formatter::string_format`'s generic `{field}`/
//! `{field:format_spec}` shorthand dialect (issue #751 generalized it
//! out of `calibre_ebooks::covers`'s original narrower port for
//! exactly this reason) against `calibre_db::formatter_functions`'s
//! real `Cache`-backed `ValueSource`/`FunctionRegistry`/
//! `FunctionCatalog` -- the same real engine `template_tester.rs`
//! (#763) already exposes for GPM, now reused here for the
//! `{field}`-shorthand dialect real upstream's own default template
//! actually uses.
//!
//! # Real, disclosed narrowing versus upstream
//!
//! - **No cover/metadata-OPF export, no plugboards.** Only the book's
//!   own format files are copied. Real upstream's `do_save_book_to_disk`
//!   also optionally writes a cover image and a `metadata.opf`
//!   sidecar, and can run each format through a device "plugboard"
//!   (per-format metadata rewrite rules) before writing -- none of
//!   that exists here. Real, separate, disclosed follow-up work.
//! - **No `--asciiize`/`--to-lowercase`/`--replace-whitespace`/`--timefmt`
//!   options.** Only the real, always-applied `sanitize_file_name` +
//!   whitespace-trim step every upstream mode shares.
//! - **Extra path-traversal hardening beyond upstream.** Real
//!   upstream's own `get_components` sanitizes each `/`-split
//!   component via `sanitize_file_name`, which strips `/` but leaves a
//!   literal `..` component untouched (it contains no character
//!   `sanitize_file_name` treats as invalid) -- a real latent
//!   directory-escape in upstream's own template evaluation if a
//!   malicious/buggy template ever produced one. This port additionally
//!   drops any component that sanitizes down to exactly `..` or empty,
//!   and checks every final output path against the destination twice:
//!   once lexically before `create_dir_all`, then again via
//!   [`std::fs::canonicalize`] after (so a symlink pre-planted
//!   *inside* the destination tree, which a lexical `starts_with` on
//!   unresolved paths can't see, is also caught) -- see
//!   `save_one_book`'s own doc.
//! - **`dest` is a deliberately arbitrary absolute filesystem path,
//!   not confined to a configured export root.** This matches this
//!   issue's own filed scope ("a destination path... let the Tauri
//!   side pick a real folder via `tauri-plugin-dialog`") and real
//!   upstream's own GUI action, which is a plain OS folder picker with
//!   no root restriction either. The route sits behind this crate's
//!   standard `auth::require_auth` middleware like every other
//!   mutating route (confirmed via `lib.rs`'s router: registered on
//!   `api` before `route_layer(require_auth)` wraps it, not on the
//!   small `auth_required=False` allowlist next to it) -- there is no
//!   per-route authorization tier beyond "authenticated" anywhere in
//!   this crate yet (every authenticated user can already add/delete/
//!   overwrite any book file), so gating this one route behind a
//!   finer-grained role would be new, cross-cutting RBAC work, not a
//!   fix scoped to this route. Real residual risk in a multi-user
//!   server deployment: an authenticated user can direct the server
//!   process to write files anywhere it has filesystem permissions
//!   for, not just under the library -- a genuinely new surface real
//!   upstream never had (its own save-to-disk is Qt-GUI-local-only,
//!   never reachable over the network). Worth a future dedicated
//!   per-route permission system if this crate ever serves untrusted
//!   multi-tenant users; out of scope for this issue.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use axum::extract::{Path as AxumPath, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use calibre_db::cache::Cache;
use calibre_db::formatter_functions::{CacheCatalog, CacheFunctions, CacheValueSource};
use calibre_utils::filenames::sanitize_file_name;
use calibre_utils::formatter::string_format;

use crate::errors::ServerError;
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct SaveToDiskBody {
    book_ids: Vec<i32>,
    template: String,
    dest: String,
    /// `None`/omitted means "every format the book actually has",
    /// matching upstream's own `opts.formats == 'all'` default.
    #[serde(default)]
    formats: Option<Vec<String>>,
    /// Port of `opts.single_dir`: keep only the template's last
    /// `/`-separated component (drop the folder structure, save
    /// straight into `dest`).
    #[serde(default)]
    single_dir: bool,
}

fn book_row(cache: &Cache, book_id: i32) -> Result<Value, String> {
    let ids: HashSet<i32> = std::iter::once(book_id).collect();
    let rows = cache.get_data_as_dict(None, true, Some(&ids), false).map_err(|e| e.to_string())?;
    rows.into_iter().next().ok_or_else(|| format!("No book with id: {book_id}"))
}

/// Port of `get_components`: evaluate `template` against the book,
/// split on `/`, sanitize and drop empty/`..` components, falling
/// back to the bare book id if nothing real survives.
fn path_components(cache: &Cache, book_id: i32, template: &str, single_dir: bool) -> Result<Vec<String>, String> {
    let value_source = CacheValueSource::new(cache, book_id).map_err(|e| e.to_string())?;
    let functions = CacheFunctions::new(cache, book_id);
    let rendered = string_format::evaluate_template(template, &value_source, &CacheCatalog, &functions)?;

    let mut components: Vec<String> = rendered.split('/').map(str::trim).filter(|s| !s.is_empty()).map(sanitize_file_name).filter(|s| !s.is_empty() && s != "..").collect();

    if components.is_empty() {
        components = vec![book_id.to_string()];
    }
    if single_dir {
        components = components.into_iter().next_back().into_iter().collect();
    }
    Ok(components)
}

/// Copies every requested (or, if none requested, every available)
/// format of one book to `dest_root`, named from `template`. Returns
/// the real paths written.
///
/// `canonical_dest_root` is `dest_root` already resolved via
/// [`std::fs::canonicalize`] by the caller (once, not per book) --
/// used below to catch a symlink planted *inside* the destination
/// tree that would otherwise let a lexically-nested `out_path`
/// resolve outside `dest_root` at write time. A plain `starts_with`
/// on unresolved paths (this function's original version) can't see
/// that: `dest_root/link -> /etc` still lexically starts with
/// `dest_root` right up until the OS follows the symlink.
fn save_one_book(cache: &Cache, book_id: i32, template: &str, dest_root: &Path, canonical_dest_root: &Path, formats: Option<&[String]>, single_dir: bool) -> Result<Vec<String>, String> {
    let row = book_row(cache, book_id)?;
    let available: Vec<String> = row["available_formats"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_uppercase())).collect()).unwrap_or_default();
    let wanted: Option<HashSet<String>> = formats.map(|f| f.iter().map(|s| s.to_uppercase()).collect());

    let components = path_components(cache, book_id, template, single_dir)?;
    let base_path = dest_root.join(components.iter().collect::<PathBuf>());

    let mut written = Vec::new();
    for fmt in &available {
        if let Some(wanted) = &wanted {
            if !wanted.contains(fmt) {
                continue;
            }
        }
        let ext = fmt.to_lowercase();
        let src_str = row.get(format!("fmt_{ext}")).and_then(|v| v.as_str()).ok_or_else(|| format!("No {ext} format for the book {book_id}"))?;
        let src = PathBuf::from(src_str);

        // Real upstream builds path components with no extension yet
        // (`last_has_extension=false`) -- a plain concatenation, not
        // `Path::with_extension` (which would mangle a filename that
        // already legitimately contains a `.`, e.g. a title like
        // "Vol. 2").
        let out_path = PathBuf::from(format!("{}.{ext}", base_path.display()));

        // Defense in depth, lexical pass: every sanitized component
        // already had `/` and bare `..` stripped above, so this should
        // never actually trip -- but a destination escape is exactly
        // the class of bug worth double-checking rather than assuming.
        if !out_path.starts_with(dest_root) {
            return Err(format!("refusing to write outside the destination: {}", out_path.display()));
        }

        let Some(parent) = out_path.parent() else {
            return Err(format!("refusing to write to a path with no parent directory: {}", out_path.display()));
        };
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;

        // Defense in depth, symlink-resolved pass: re-check *after*
        // `create_dir_all` (so `canonicalize` has something real to
        // resolve) that the directory we're about to write into still
        // really lives under `canonical_dest_root` once every symlink
        // is followed -- catches a symlink pre-planted somewhere
        // inside an already-existing destination tree, which the
        // lexical check above cannot see.
        let canonical_parent = std::fs::canonicalize(parent).map_err(|e| e.to_string())?;
        if !canonical_parent.starts_with(canonical_dest_root) {
            return Err(format!("refusing to write outside the destination (symlink escape detected): {}", canonical_parent.display()));
        }

        let real_out_path = canonical_parent.join(out_path.file_name().expect("out_path always has a file name -- it's built by appending an extension"));
        std::fs::copy(&src, &real_out_path).map_err(|e| e.to_string())?;
        written.push(real_out_path.display().to_string());
    }

    if written.is_empty() {
        return Err(format!("no requested format is available for book {book_id} (available: {available:?})"));
    }
    Ok(written)
}

/// `POST /save-to-disk/{library_id}`.
pub async fn save_to_disk(State(state): State<AppState>, AxumPath(library_id): AxumPath<String>, Json(body): Json<SaveToDiskBody>) -> Result<Json<Value>, ServerError> {
    let cache = state.cache_for(Some(&library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))?;
    if body.book_ids.is_empty() {
        return Err(ServerError::BadRequest("book_ids must not be empty".to_string()));
    }
    let dest_root = PathBuf::from(&body.dest);
    if !dest_root.is_absolute() {
        return Err(ServerError::BadRequest("dest must be an absolute path".to_string()));
    }

    let results = tokio::task::spawn_blocking(move || -> Result<Vec<Value>, String> {
        std::fs::create_dir_all(&dest_root).map_err(|e| e.to_string())?;
        // Canonicalize once, outside the per-book loop -- see
        // `save_one_book`'s own doc for why this (not a lexical
        // `starts_with`) is what actually catches a symlink planted
        // inside the destination tree.
        let canonical_dest_root = std::fs::canonicalize(&dest_root).map_err(|e| e.to_string())?;
        Ok(body
            .book_ids
            .iter()
            .map(|&book_id| match save_one_book(&cache, book_id, &body.template, &dest_root, &canonical_dest_root, body.formats.as_deref(), body.single_dir) {
                Ok(paths) => json!({"book_id": book_id, "ok": true, "paths": paths}),
                Err(error) => json!({"book_id": book_id, "ok": false, "error": error}),
            })
            .collect())
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(ServerError::InternalServerError)?;

    Ok(Json(json!({"results": results})))
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
        std::fs::write(&source, b"hello world").unwrap();
        let mut meta = calibre_ebooks::metadata::MetaInformation::default();
        meta.title = "My Book".to_string();
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
            tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()), news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()), tts_voice: None, plugin_store: None, plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(calibre_customize::registry::PluginRegistry::new())),
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
    async fn a_real_book_is_exported_to_a_real_templated_path() {
        let (_dir, router, book_id) = test_app();
        let dest = tempfile::tempdir().unwrap();
        let (status, body) = post_json(
            &router,
            "/save-to-disk/default",
            serde_json::json!({"book_ids": [book_id], "template": "{authors}/{title}", "dest": dest.path().to_string_lossy()}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let result = &body["results"][0];
        assert_eq!(result["ok"], true, "{result}");
        let paths = result["paths"].as_array().unwrap();
        assert_eq!(paths.len(), 1);
        let written = std::path::PathBuf::from(paths[0].as_str().unwrap());
        assert!(written.starts_with(dest.path()), "{written:?} should be under {:?}", dest.path());
        assert_eq!(std::fs::read_to_string(&written).unwrap(), "hello world");
        assert!(written.to_string_lossy().contains("Jane Doe"));
        assert!(written.to_string_lossy().contains("My Book"));
    }

    #[tokio::test]
    async fn a_dotdot_template_component_is_dropped_not_used_as_a_real_path_segment() {
        let (_dir, router, book_id) = test_app();
        let dest = tempfile::tempdir().unwrap();
        let (status, body) = post_json(&router, "/save-to-disk/default", serde_json::json!({"book_ids": [book_id], "template": "../../etc/{title}", "dest": dest.path().to_string_lossy()})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let result = &body["results"][0];
        assert_eq!(result["ok"], true, "{result}");
        let written = std::path::PathBuf::from(result["paths"][0].as_str().unwrap());
        assert!(written.starts_with(dest.path()), "the `..`/`etc` segments must not have escaped dest: {written:?}");
    }

    #[tokio::test]
    async fn a_symlink_planted_inside_dest_cannot_be_used_to_escape_it() {
        // A lexical `out_path.starts_with(dest_root)` check alone would
        // pass here (`dest/escape/...` really does start with `dest`
        // as *strings*) -- only resolving the symlink via
        // `canonicalize` reveals it actually lands under `outside`,
        // which this route must refuse to write into.
        let (_dir, router, book_id) = test_app();
        let dest = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dest.path().join("escape")).unwrap();

        let (status, body) = post_json(&router, "/save-to-disk/default", serde_json::json!({"book_ids": [book_id], "template": "escape/{title}", "dest": dest.path().to_string_lossy()})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let result = &body["results"][0];
        assert_eq!(result["ok"], false, "a symlink escape should be refused, not silently written: {result}");
        assert!(result["error"].as_str().unwrap().contains("symlink escape"), "{result}");
        assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none(), "nothing should have been written into the symlink target");
    }

    #[tokio::test]
    async fn only_the_requested_format_is_written_when_formats_is_given() {
        let (_dir, router, book_id) = test_app();
        let dest = tempfile::tempdir().unwrap();
        let (status, body) = post_json(
            &router,
            "/save-to-disk/default",
            serde_json::json!({"book_ids": [book_id], "template": "{title}", "dest": dest.path().to_string_lossy(), "formats": ["MOBI"]}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let result = &body["results"][0];
        assert_eq!(result["ok"], false, "{result}");
        assert!(result["error"].as_str().unwrap().contains("no requested format"), "{result}");
    }

    #[tokio::test]
    async fn a_relative_dest_is_rejected() {
        let (_dir, router, book_id) = test_app();
        let (status, _) = post_json(&router, "/save-to-disk/default", serde_json::json!({"book_ids": [book_id], "template": "{title}", "dest": "relative/path"})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn an_empty_book_ids_list_is_rejected() {
        let (_dir, router, _book_id) = test_app();
        let dest = tempfile::tempdir().unwrap();
        let (status, _) = post_json(&router, "/save-to-disk/default", serde_json::json!({"book_ids": [], "template": "{title}", "dest": dest.path().to_string_lossy()})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn an_unknown_book_id_reports_a_per_book_error_not_a_500() {
        let (_dir, router, _book_id) = test_app();
        let dest = tempfile::tempdir().unwrap();
        let (status, body) = post_json(&router, "/save-to-disk/default", serde_json::json!({"book_ids": [999999], "template": "{title}", "dest": dest.path().to_string_lossy()})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["results"][0]["ok"], false, "{body}");
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
            tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()), news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()), tts_voice: None, plugin_store: None, plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(calibre_customize::registry::PluginRegistry::new())),
        };
        let router = crate::test_router(state);
        (src_dir, dest_dir, broker, router, book_id)
    }

    #[tokio::test]
    async fn save_to_disk_404s_for_an_unknown_library_id() {
        let (_src_dir, dest_dir, _broker, router, book_id) = test_app_with_two_libraries();
        let (status, _) = post_json(&router, &format!("/save-to-disk/no-such-library"), serde_json::json!({"book_ids": [book_id], "template": "{title}", "dest": dest_dir.path().to_string_lossy()})).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
