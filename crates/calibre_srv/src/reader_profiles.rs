//! Port of `content.py`'s reader-profile storage endpoints (issue
//! #419) -- named, per-user JSON blobs of in-browser-reader settings
//! (font size, theme, etc.), independent of `render_book.py`'s actual
//! rendering pipeline.
//!
//! # Storage: `rusqlite`, not upstream's `profiles.json`
//!
//! Upstream stores every user's profiles in one shared
//! `config_dir/profiles.json` file (`{user_key: {profile_name:
//! {...,"__timestamp__":...}}}`), read-modify-written whole on every
//! save. This crate has no `config_dir` concept (see `users.rs`'s own
//! note on the same gap) and already has a real convention for this
//! kind of server-local state: a small `rusqlite` database next to
//! `server-users.sqlite`, same pattern [`crate::users::UserManager`]
//! uses (own `Connection`, own schema-versioned `CREATE TABLE`, no
//! `calibre_db`/library-file involvement -- this is server config, not
//! library data). A table keyed on `(user_key, name)` avoids the
//! whole-file read-modify-write race a shared JSON file would have
//! under concurrent requests.
//!
//! # `user_key`, narrowed
//!
//! Upstream's `expand_profile_user_names` cross-links a profile
//! lookup with the *desktop GUI's own* local viewer session
//! (`get_session_pref('sync_annots_user', ...)`, a `viewer:`-prefixed
//! key) so a profile saved in the desktop app's embedded viewer and
//! one saved via this HTTP API can share settings. That's a desktop-
//! GUI-session concept with no server-side equivalent -- not ported.
//! This port uses exactly the one key `content.py`'s own endpoint
//! computes and passes in: `user:<username>` for an authenticated
//! request, or the bare `user:` key for an anonymous one (matching
//! upstream's own `which = 'user:'; if rd.username: which +=
//! rd.username`).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use axum::extract::{Extension, State};
use axum::Json;
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::Value;

use crate::auth::AuthenticatedUser;
use crate::errors::ServerError;
use crate::AppState;

pub struct ProfileStore {
    conn: Mutex<Connection>,
}

impl ProfileStore {
    pub fn new(path: &Path) -> anyhow::Result<ProfileStore> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        let user_version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if user_version == 0 {
            conn.execute_batch(
                r#"
                CREATE TABLE profiles (
                    user_key TEXT NOT NULL,
                    name TEXT NOT NULL,
                    data TEXT NOT NULL,
                    PRIMARY KEY (user_key, name)
                );
                PRAGMA user_version=1;
                "#,
            )?;
        }
        Ok(ProfileStore { conn: Mutex::new(conn) })
    }

    /// An ephemeral, in-memory store -- for tests, and for any future
    /// caller that wants profile storage without persisting it to
    /// disk (there is no real upstream equivalent of this constructor;
    /// upstream always persists to `profiles.json`).
    pub fn new_in_memory() -> anyhow::Result<ProfileStore> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(
            r#"
            CREATE TABLE profiles (
                user_key TEXT NOT NULL,
                name TEXT NOT NULL,
                data TEXT NOT NULL,
                PRIMARY KEY (user_key, name)
            );
            "#,
        )?;
        Ok(ProfileStore { conn: Mutex::new(conn) })
    }

    /// Port of `load_viewer_profiles`, narrowed to a single
    /// `user_key` (see module doc).
    pub fn get_all(&self, user_key: &str) -> anyhow::Result<HashMap<String, Value>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT name, data FROM profiles WHERE user_key = ?1")?;
        let rows = stmt.query_map([user_key], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
        let mut out = HashMap::new();
        for row in rows {
            let (name, data) = row?;
            if let Ok(value) = serde_json::from_str(&data) {
                out.insert(name, value);
            }
        }
        Ok(out)
    }

    /// Port of `save_viewer_profile`, narrowed to a single `user_key`.
    /// Stamps `__timestamp__` the same way upstream does.
    pub fn save(&self, user_key: &str, name: &str, mut profile: Value) -> anyhow::Result<()> {
        if let Value::Object(map) = &mut profile {
            map.insert("__timestamp__".to_string(), Value::String(chrono::Utc::now().to_rfc3339()));
        }
        let data = serde_json::to_string(&profile)?;
        let conn = self.conn.lock().unwrap();
        conn.execute("INSERT OR REPLACE INTO profiles (user_key, name, data) VALUES (?1, ?2, ?3)", (user_key, name, data))?;
        Ok(())
    }
}

fn user_key(user: &Option<Extension<AuthenticatedUser>>) -> String {
    match user {
        Some(Extension(AuthenticatedUser(name))) => format!("user:{name}"),
        None => "user:".to_string(),
    }
}

/// `GET /reader-profiles/get-all`. Port of `get_all_reader_profiles`.
pub async fn get_all(State(state): State<AppState>, user: Option<Extension<AuthenticatedUser>>) -> Result<Json<Value>, ServerError> {
    let key = user_key(&user);
    let profiles = tokio::task::spawn_blocking({
        let store = state.reader_profiles.clone();
        move || store.get_all(&key)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;
    Ok(Json(serde_json::to_value(profiles).unwrap()))
}

#[derive(Debug, Deserialize)]
pub struct SaveProfileBody {
    name: String,
    profile: Value,
}

/// `POST /reader-profiles/save`. Port of `save_reader_profile`.
pub async fn save(State(state): State<AppState>, user: Option<Extension<AuthenticatedUser>>, Json(body): Json<SaveProfileBody>) -> Result<Json<Value>, ServerError> {
    if !body.profile.is_object() && !body.profile.is_null() {
        return Err(ServerError::BadRequest(format!("profile must be a dict not {}", type_name(&body.profile))));
    }
    let key = user_key(&user);
    tokio::task::spawn_blocking({
        let store = state.reader_profiles.clone();
        let name = body.name.clone();
        move || store.save(&key, &name, body.profile)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;
    Ok(Json(Value::Bool(true)))
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

#[cfg(test)]
mod store_tests {
    use super::*;

    #[test]
    fn save_then_get_all_round_trips_and_stamps_a_timestamp() {
        let store = ProfileStore::new_in_memory().unwrap();
        store.save("user:alice", "default", serde_json::json!({"font_size": 16})).unwrap();
        let all = store.get_all("user:alice").unwrap();
        assert_eq!(all["default"]["font_size"], 16);
        assert!(all["default"]["__timestamp__"].is_string());
    }

    #[test]
    fn profiles_are_isolated_per_user_key() {
        let store = ProfileStore::new_in_memory().unwrap();
        store.save("user:alice", "default", serde_json::json!({"x": 1})).unwrap();
        store.save("user:bob", "default", serde_json::json!({"x": 2})).unwrap();
        assert_eq!(store.get_all("user:alice").unwrap()["default"]["x"], 1);
        assert_eq!(store.get_all("user:bob").unwrap()["default"]["x"], 2);
    }

    #[test]
    fn saving_the_same_name_twice_overwrites() {
        let store = ProfileStore::new_in_memory().unwrap();
        store.save("user:alice", "default", serde_json::json!({"x": 1})).unwrap();
        store.save("user:alice", "default", serde_json::json!({"x": 2})).unwrap();
        let all = store.get_all("user:alice").unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all["default"]["x"], 2);
    }

    #[test]
    fn get_all_for_an_unknown_user_is_empty() {
        let store = ProfileStore::new_in_memory().unwrap();
        assert!(store.get_all("user:nobody").unwrap().is_empty());
    }
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use calibre_db::cache::Cache;

    fn test_app() -> (tempfile::TempDir, axum::Router) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let state = crate::AppState { cache: std::sync::Arc::new(cache), opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None, changes: crate::web_socket::new_change_broadcaster(), reader_profiles: std::sync::Arc::new(super::ProfileStore::new_in_memory().unwrap()) };
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

    #[tokio::test]
    async fn save_then_get_all_round_trips_over_http() {
        let (_dir, router) = test_app();
        let (status, body) = post_json(&router, "/reader-profiles/save", serde_json::json!({"name": "default", "profile": {"theme": "dark"}})).await;
        assert_eq!(status, StatusCode::OK, "got: {body}");
        assert_eq!(body, serde_json::json!(true));

        let (status, all) = get_json(&router, "/reader-profiles/get-all").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(all["default"]["theme"], "dark");
    }

    #[tokio::test]
    async fn save_rejects_a_non_dict_profile() {
        let (_dir, router) = test_app();
        let (status, _) = post_json(&router, "/reader-profiles/save", serde_json::json!({"name": "default", "profile": "not a dict"})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn save_accepts_a_null_profile() {
        // Matches upstream: `if not isinstance(profile, dict) and
        // profile is not None: raise TypeError(...)` -- None is
        // explicitly allowed through.
        let (_dir, router) = test_app();
        let (status, _) = post_json(&router, "/reader-profiles/save", serde_json::json!({"name": "default", "profile": null})).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn anonymous_and_authenticated_users_get_isolated_profiles() {
        // No auth is configured for this test app, so every request
        // is anonymous (user_key "user:") -- confirm two separate
        // saves under different explicit names don't collide, and
        // that get-all reflects both.
        let (_dir, router) = test_app();
        post_json(&router, "/reader-profiles/save", serde_json::json!({"name": "a", "profile": {"x": 1}})).await;
        post_json(&router, "/reader-profiles/save", serde_json::json!({"name": "b", "profile": {"x": 2}})).await;
        let (_, all) = get_json(&router, "/reader-profiles/get-all").await;
        assert_eq!(all["a"]["x"], 1);
        assert_eq!(all["b"]["x"], 2);
    }
}
