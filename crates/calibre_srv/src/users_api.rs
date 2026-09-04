//! Port of `calibre.srv.users_api`: `POST /users/change-pw`, letting an
//! authenticated user change their own password.
//!
//! # Not ported
//!
//! `is_allowed_to_change_password_via_http` -- `users.rs`'s
//! `UserManager` doesn't expose the `misc_data` column that flag lives
//! in yet (see that module's own doc), so this port skips the check
//! rather than half-implementing it against a column nothing else
//! reads either; every authenticated user can change their own
//! password via this endpoint for now.

use axum::extract::{Extension, State};
use axum::Json;
use serde::Deserialize;

use crate::auth::AuthenticatedUser;
use crate::errors::ServerError;
use crate::users::validate_password;
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct ChangePasswordRequest {
    oldpw: String,
    newpw: String,
}

/// Port of `change_pw`. Requires [`AuthenticatedUser`] (i.e. this
/// route must sit behind `auth::require_auth`, which it does -- see
/// `lib.rs::router`) -- upstream's own `if user is None: raise
/// HTTPForbidden(...)` anonymous-user check is therefore unreachable
/// here rather than re-implemented: with no valid `Authorization`
/// header, `require_auth` itself already rejects the request with
/// `401` before this handler ever runs.
pub async fn change_pw(
    State(state): State<AppState>,
    Extension(AuthenticatedUser(username)): Extension<AuthenticatedUser>,
    Json(body): Json<ChangePasswordRequest>,
) -> Result<String, ServerError> {
    let Some(gate) = &state.auth else {
        return Err(ServerError::Forbidden("Authentication is not enabled on this server".to_string()));
    };
    if gate.users.get(&username).as_deref() != Some(body.oldpw.as_str()) {
        return Err(ServerError::BadRequest("Existing password is incorrect".to_string()));
    }
    if let Some(err) = validate_password(&body.newpw) {
        return Err(ServerError::BadRequest(err));
    }
    gate.users.change_password(&username, &body.newpw).map_err(ServerError::BadRequest)?;
    Ok(format!("password for {username} changed"))
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{header, Request, StatusCode};
    use std::sync::Arc;
    use tower::ServiceExt;

    use crate::auth::AuthGate;
    use crate::opts::ServerOptions;
    use crate::users::UserManager;
    use crate::AppState;

    fn basic_auth_header(username: &str, password: &str) -> String {
        use base64::Engine;
        format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}")))
    }

    fn test_state() -> (tempfile::TempDir, AppState) {
        let dir = tempfile::tempdir().unwrap();
        let cache = calibre_db::cache::Cache::new(dir.path()).unwrap();
        let users = UserManager::new(&dir.path().join("users.sqlite")).unwrap();
        users.add_user("alice", "hunter2", false).unwrap();
        let auth = Some(Arc::new(AuthGate::new(users, "calibre".to_string(), 0, 5)));
        (dir, AppState { libraries: None, cache: Arc::new(cache), opts: Arc::new(ServerOptions::default()), auth, changes: crate::web_socket::new_change_broadcaster(), reader_profiles: Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()), book_cache: Arc::new(crate::books_cache::BookCache::open_temp()), jobs: Arc::new(crate::jobs::JobsManager::new(4, std::time::Duration::from_secs(3600))), render_jobs: Arc::new(crate::render_endpoints::RenderJobRegistry::new()) })
    }

    #[tokio::test]
    async fn change_pw_updates_the_password_when_the_old_one_is_correct() {
        let (_dir, state) = test_state();
        let router = crate::test_router(state.clone());
        let req = Request::builder()
            .method("POST")
            .uri("/users/change-pw")
            .header(header::AUTHORIZATION, basic_auth_header("alice", "hunter2"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"oldpw":"hunter2","newpw":"new-password"}"#))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(state.auth.as_ref().unwrap().users.get("alice"), Some("new-password".to_string()));
    }

    #[tokio::test]
    async fn change_pw_rejects_an_incorrect_old_password() {
        let (_dir, state) = test_state();
        let router = crate::test_router(state.clone());
        let req = Request::builder()
            .method("POST")
            .uri("/users/change-pw")
            .header(header::AUTHORIZATION, basic_auth_header("alice", "hunter2"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"oldpw":"wrong","newpw":"new-password"}"#))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("incorrect"));
    }

    #[tokio::test]
    async fn change_pw_requires_authentication() {
        let (_dir, state) = test_state();
        let router = crate::test_router(state);
        let req = Request::builder()
            .method("POST")
            .uri("/users/change-pw")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"oldpw":"hunter2","newpw":"new-password"}"#))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
