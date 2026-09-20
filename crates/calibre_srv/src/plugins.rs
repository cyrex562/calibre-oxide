//! Plugin management routes (issue #801, closing the #754 epic).
//!
//! Real upstream has this as a Preferences panel plus a
//! `plugin_updater.py` dialog. This port exposes it over HTTP, since
//! this crate's content server is the only real UI the project has.
//!
//! # The capability disclosure is the point
//!
//! `GET /plugins/list` reports each plugin's **declared capabilities**
//! (`allowed_hosts` / `allowed_paths`) and whether it is fully
//! sandboxed, and `POST /plugins/inspect` reports the same for a
//! package that has **not been installed yet**.
//!
//! That inspect-before-install step is the whole reason the sandbox is
//! worth having from a user's point of view: it turns "grant this
//! third-party code network access" into a visible, informed decision.
//! Upstream cannot offer anything equivalent -- its plugins get the
//! user's full privileges unconditionally, so there is nothing to
//! disclose and no decision to make.
//!
//! # Enabled/disabled state is per-process, deliberately
//!
//! Enable/disable lives in the in-memory
//! [`calibre_customize::registry::PluginRegistry`], not on disk.
//! Persisting it is real, separable work (it needs a settings store and
//! a decision about whether state is per-user or per-server); leaving
//! it in memory keeps this issue honest rather than half-implementing
//! persistence. Documented on the route so it is not mistaken for a
//! bug.

use axum::extract::{Path as AxumPath, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use calibre_plugins_wasm::{PluginPackage, PluginStore};

use crate::errors::ServerError;
use crate::AppState;

fn store(state: &AppState) -> Result<&PluginStore, ServerError> {
    state
        .plugin_store
        .as_deref()
        .ok_or_else(|| ServerError::ServiceUnavailable("plugins are not enabled on this server -- start it with --plugin-dir <path>".to_string()))
}

/// Serializes a package for the UI, including everything needed to
/// make an informed install/enable decision.
fn package_json(pkg: &PluginPackage, enabled: bool) -> Value {
    let caps = &pkg.manifest.capabilities;
    json!({
        "name": pkg.manifest.name,
        "version": pkg.manifest.version,
        "author": pkg.manifest.author,
        "description": pkg.manifest.description,
        "plugin_type": pkg.manifest.plugin_type,
        "file_types": pkg.manifest.file_types,
        "enabled": enabled,
        "capabilities": {
            "allowed_hosts": caps.allowed_hosts,
            "allowed_paths": caps.allowed_paths,
            "fully_sandboxed": caps.is_fully_sandboxed(),
        },
        "limits": {
            "timeout_ms": pkg.manifest.limits.timeout_ms,
            "max_pages": pkg.manifest.limits.max_pages,
        },
    })
}

/// `GET /plugins/list`.
pub async fn list(State(state): State<AppState>) -> Result<Json<Value>, ServerError> {
    let store = store(&state)?;
    let packages = store.list().map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    let registry = state.plugin_registry.lock().expect("plugin registry lock poisoned");
    let plugins: Vec<Value> = packages
        .iter()
        // A plugin absent from the registry has never been loaded into
        // it; treat it as enabled, matching a fresh install.
        .map(|p| package_json(p, registry.is_enabled(&p.manifest.name).unwrap_or(true)))
        .collect();

    Ok(Json(json!({ "plugins": plugins })))
}

#[derive(Debug, Deserialize)]
pub struct PathBody {
    pub path: String,
}

/// `POST /plugins/inspect` -- read a package's manifest **without
/// installing it**, so the UI can show what it will be granted before
/// the user commits.
pub async fn inspect(State(state): State<AppState>, Json(body): Json<PathBody>) -> Result<Json<Value>, ServerError> {
    // Requires plugins to be enabled even though it only reads: this
    // route opens a caller-supplied path, so it must sit behind the
    // same opt-in as the rest of the plugin surface.
    store(&state)?;
    let pkg = PluginPackage::read_zip(std::path::Path::new(&body.path)).map_err(|e| ServerError::BadRequest(e.to_string()))?;
    Ok(Json(package_json(&pkg, true)))
}

/// `POST /plugins/install`.
pub async fn install(State(state): State<AppState>, Json(body): Json<PathBody>) -> Result<Json<Value>, ServerError> {
    let store = store(&state)?;
    let pkg = store.install(std::path::Path::new(&body.path)).map_err(|e| ServerError::BadRequest(e.to_string()))?;
    Ok(Json(package_json(&pkg, true)))
}

/// `POST /plugins/remove/{name}`.
pub async fn remove(State(state): State<AppState>, AxumPath(name): AxumPath<String>) -> Result<Json<Value>, ServerError> {
    let store = store(&state)?;
    store.remove(&name).map_err(|e| ServerError::NotFound(e.to_string()))?;
    Ok(Json(json!({"ok": true})))
}

#[derive(Debug, Deserialize)]
pub struct SetEnabledBody {
    pub enabled: bool,
}

/// `POST /plugins/set-enabled/{name}`.
///
/// See the module doc: this state is per-process and not persisted.
pub async fn set_enabled(State(state): State<AppState>, AxumPath(name): AxumPath<String>, Json(body): Json<SetEnabledBody>) -> Result<Json<Value>, ServerError> {
    let store = store(&state)?;
    if !store.is_installed(&name) {
        return Err(ServerError::NotFound(format!("no plugin named {name:?} is installed")));
    }

    let mut registry = state.plugin_registry.lock().expect("plugin registry lock poisoned");
    match registry.set_enabled(&name, body.enabled) {
        Ok(()) => Ok(Json(json!({"ok": true, "enabled": body.enabled}))),
        // Honor `can_be_disabled` as a real refusal rather than a
        // silent no-op, so the UI can say why instead of showing a
        // toggle that appears to work and doesn't.
        Err(calibre_customize::registry::RegistryError::CannotBeDisabled(n)) => {
            Err(ServerError::BadRequest(format!("the plugin {n:?} declares that it cannot be disabled")))
        }
        // Not yet loaded into this process's registry -- nothing to
        // toggle, but the package is installed, so report it as such.
        Err(calibre_customize::registry::RegistryError::NotFound(_)) => {
            Err(ServerError::BadRequest(format!("the plugin {name:?} is installed but not loaded in this server process")))
        }
        Err(e) => Err(ServerError::InternalServerError(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use calibre_db::cache::Cache;

    fn write_package(dir: &std::path::Path, file: &str, name: &str, extra: &str) -> std::path::PathBuf {
        let json = format!(
            r#"{{"abi_version": 1, "name": "{name}", "version": "1.2.3", "author": "A Third Party",
                 "description": "does a thing", "plugin_type": "file_type",
                 "wasm": "p.wasm", "file_types": ["txt"]{extra}}}"#
        );
        let path = dir.join(file);
        let f = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(f);
        let opts = zip::write::FileOptions::default();
        zip.start_file("calibre-plugin.json", opts).unwrap();
        zip.write_all(json.as_bytes()).unwrap();
        zip.start_file("p.wasm", opts).unwrap();
        zip.write_all(b"\0asm\x01\x00\x00\x00").unwrap();
        zip.finish().unwrap();
        path
    }

    /// Builds a router with plugins either enabled (a real store) or
    /// not, so the "plugins are off" path is covered too.
    fn test_app(with_plugins: bool) -> (tempfile::TempDir, axum::Router) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let plugin_store =
            with_plugins.then(|| std::sync::Arc::new(calibre_plugins_wasm::PluginStore::open(dir.path().join("plugins")).unwrap()));

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
            plugin_store,
            plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(calibre_customize::registry::PluginRegistry::new())),
        };
        (dir, crate::test_router(state))
    }

    async fn post_json(router: &axum::Router, uri: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let req = Request::builder().method("POST").uri(uri).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }

    async fn get_json(router: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }

    #[tokio::test]
    async fn every_route_reports_service_unavailable_when_plugins_are_not_enabled() {
        let (_dir, router) = test_app(false);
        for (method, uri) in [("GET", "/plugins/list"), ("POST", "/plugins/install"), ("POST", "/plugins/remove/x")] {
            let req = Request::builder().method(method).uri(uri).header("content-type", "application/json").body(Body::from(r#"{"path":"/x"}"#)).unwrap();
            let resp = router.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE, "{method} {uri}");
        }
    }

    #[tokio::test]
    async fn listing_is_empty_before_anything_is_installed() {
        let (_dir, router) = test_app(true);
        let (status, body) = get_json(&router, "/plugins/list").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["plugins"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn a_real_package_installs_and_then_lists_with_its_metadata() {
        let (dir, router) = test_app(true);
        let src = write_package(dir.path(), "src.zip", "Alpha Plugin", "");

        let (status, installed) = post_json(&router, "/plugins/install", serde_json::json!({"path": src})).await;
        assert_eq!(status, StatusCode::OK, "{installed}");
        assert_eq!(installed["name"], "Alpha Plugin");
        assert_eq!(installed["version"], "1.2.3");
        assert_eq!(installed["author"], "A Third Party");

        let (_status, body) = get_json(&router, "/plugins/list").await;
        let plugins = body["plugins"].as_array().unwrap();
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0]["name"], "Alpha Plugin");
        assert_eq!(plugins[0]["plugin_type"], "file_type");
    }

    #[tokio::test]
    async fn a_plugin_that_wants_nothing_is_reported_as_fully_sandboxed() {
        let (dir, router) = test_app(true);
        let src = write_package(dir.path(), "src.zip", "Harmless", "");
        post_json(&router, "/plugins/install", serde_json::json!({"path": src})).await;

        let (_s, body) = get_json(&router, "/plugins/list").await;
        let caps = &body["plugins"][0]["capabilities"];
        assert_eq!(caps["fully_sandboxed"], true);
        assert_eq!(caps["allowed_hosts"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn a_plugin_requesting_network_access_discloses_exactly_which_hosts() {
        // The security-relevant assertion: a user must be able to see
        // what a plugin will be allowed to reach.
        let (dir, router) = test_app(true);
        let src = write_package(dir.path(), "net.zip", "Needs Network", r#", "capabilities": {"allowed_hosts": ["www.googleapis.com"]}"#);
        post_json(&router, "/plugins/install", serde_json::json!({"path": src})).await;

        let (_s, body) = get_json(&router, "/plugins/list").await;
        let caps = &body["plugins"][0]["capabilities"];
        assert_eq!(caps["fully_sandboxed"], false);
        assert_eq!(caps["allowed_hosts"][0], "www.googleapis.com");
    }

    #[tokio::test]
    async fn inspect_discloses_capabilities_without_installing_anything() {
        // The whole point of inspect: an informed decision BEFORE the
        // package lands in the store.
        let (dir, router) = test_app(true);
        let src = write_package(dir.path(), "net.zip", "Wants The Net", r#", "capabilities": {"allowed_hosts": ["example.com"]}"#);

        let (status, inspected) = post_json(&router, "/plugins/inspect", serde_json::json!({"path": src})).await;
        assert_eq!(status, StatusCode::OK, "{inspected}");
        assert_eq!(inspected["name"], "Wants The Net");
        assert_eq!(inspected["capabilities"]["allowed_hosts"][0], "example.com");
        assert_eq!(inspected["capabilities"]["fully_sandboxed"], false);

        let (_s, body) = get_json(&router, "/plugins/list").await;
        assert_eq!(body["plugins"].as_array().unwrap().len(), 0, "inspect must NOT have installed the package");
    }

    #[tokio::test]
    async fn inspecting_a_malformed_package_is_a_400_not_a_500() {
        let (dir, router) = test_app(true);
        let bad = dir.path().join("bad.zip");
        std::fs::write(&bad, b"not a zip").unwrap();

        let (status, _) = post_json(&router, "/plugins/inspect", serde_json::json!({"path": bad})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn installing_the_same_plugin_twice_is_refused() {
        let (dir, router) = test_app(true);
        let src = write_package(dir.path(), "src.zip", "Alpha", "");
        post_json(&router, "/plugins/install", serde_json::json!({"path": src})).await;

        let second = write_package(dir.path(), "src2.zip", "Alpha", "");
        let (status, _) = post_json(&router, "/plugins/install", serde_json::json!({"path": second})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_removed_plugin_really_disappears_from_the_listing() {
        let (dir, router) = test_app(true);
        let src = write_package(dir.path(), "src.zip", "Alpha", "");
        post_json(&router, "/plugins/install", serde_json::json!({"path": src})).await;

        let (status, _) = post_json(&router, "/plugins/remove/Alpha", serde_json::json!({})).await;
        assert_eq!(status, StatusCode::OK);

        let (_s, body) = get_json(&router, "/plugins/list").await;
        assert_eq!(body["plugins"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn removing_something_not_installed_is_a_404() {
        let (_dir, router) = test_app(true);
        let (status, _) = post_json(&router, "/plugins/remove/Ghost", serde_json::json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn toggling_a_plugin_that_is_not_installed_is_a_404() {
        let (_dir, router) = test_app(true);
        let (status, _) = post_json(&router, "/plugins/set-enabled/Ghost", serde_json::json!({"enabled": false})).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
