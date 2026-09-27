//! Orphan and ignore-list resolution over HTTP (#921).
//!
//! An orphan is a book whose file is gone -- renamed, moved out, or
//! deleted outside the app. It is the one state the tracked-in-place
//! model (#889) *requires* the user to act on, and
//! `calibre_db::orphans` has had the three resolutions the design
//! promises since #895 with nothing able to call them.
//!
//! The three, and how they map onto routes:
//!
//! | resolution | route |
//! |---|---|
//! | point at a file already in the library | `POST /orphans/relocate/...` |
//! | copy the file back in | `POST /orphans/upload/...` |
//! | drop the entry | `POST /orphans/forget/...` |
//!
//! `relocate` insists the chosen file is inside the library, so the
//! browser flow is "pick one of the untracked files the check already
//! found" rather than an upload -- `upload` exists for the case where
//! the file is genuinely somewhere else and has to be brought in.
//!
//! The ignore list (#896) is here too because it is the same kind of
//! thing: state the user has to be able to see and undo. A hidden list
//! of files the app refuses to show is its own kind of bug.

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::errors::ServerError;
use crate::AppState;

fn cache_or_404(state: &AppState, library_id: &str) -> Result<std::sync::Arc<calibre_db::cache::Cache>, ServerError> {
    state.cache_for(Some(library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))
}

/// `GET /orphans/{library_id}` -- every book entry with no file behind it.
pub async fn list(State(state): State<AppState>, Path(library_id): Path<String>) -> Result<Json<Value>, ServerError> {
    let cache = cache_or_404(&state, &library_id)?;
    let orphans = tokio::task::spawn_blocking(move || cache.orphans().list())
        .await
        .map_err(|e| ServerError::InternalServerError(e.to_string()))?
        .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    Ok(Json(json!(orphans
        .iter()
        .map(|o| json!({
            "book_id": o.book_id,
            "format": o.format,
            // Worth showing: "it used to be in Receipts/" is often all
            // somebody needs to find it again.
            "last_known_path": o.last_known_path,
            "noticed_at": o.noticed_at,
        }))
        .collect::<Vec<_>>())))
}

#[derive(Deserialize)]
pub struct RelocateBody {
    /// Relative to the library root. Must name a file already inside it
    /// -- see `calibre_db::orphans::relocate`, which refuses anything
    /// else rather than silently copying a file in.
    pub path: String,
}

/// Describes a [`calibre_db::orphans::RelocateOutcome`] for a person.
///
/// `ContentDiffers` is the interesting one and must reach the user: they
/// may be supplying a re-downloaded copy, or they may have picked the
/// wrong file, and only they can tell which.
fn describe(outcome: calibre_db::orphans::RelocateOutcome) -> (&'static str, &'static str) {
    use calibre_db::orphans::RelocateOutcome::*;
    match outcome {
        ContentMatches => ("content_matches", "This is the same file that was recorded."),
        ContentDiffers => ("content_differs", "This file is not the one that was recorded. If you re-downloaded or repaired it that is expected — otherwise check you picked the right file."),
        NothingToCompare => ("nothing_to_compare", "No checksum was recorded for the old file, so this could not be verified."),
    }
}

/// `POST /orphans/relocate/{book_id}/{format}/{library_id}`.
pub async fn relocate(State(state): State<AppState>, Path((book_id, format, library_id)): Path<(i32, String, String)>, Json(body): Json<RelocateBody>) -> Result<Json<Value>, ServerError> {
    let cache = cache_or_404(&state, &library_id)?;

    let outcome = tokio::task::spawn_blocking(move || -> anyhow::Result<calibre_db::orphans::RelocateOutcome> {
        let chosen = cache.backend.library_path.join(&body.path);
        calibre_db::orphans::relocate(&cache, book_id, &format, &chosen)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    // A path outside the library, or one that is not a file, is the
    // caller's mistake rather than a server fault -- and the message
    // says what to do about it, so it is worth passing through.
    .map_err(|e| ServerError::BadRequest(e.to_string()))?;

    let (code, message) = describe(outcome);
    Ok(Json(json!({"outcome": code, "message": message})))
}

/// `POST /orphans/upload/{book_id}/{format}/{library_id}/{filename}`.
///
/// The "copy the file back in" resolution. The bytes are written to the
/// library root under `filename` and the entry is pointed at them.
pub async fn upload(State(state): State<AppState>, Path((book_id, format, library_id, filename)): Path<(i32, String, String, String)>, body: Bytes) -> Result<Json<Value>, ServerError> {
    let cache = cache_or_404(&state, &library_id)?;
    if body.is_empty() {
        return Err(ServerError::BadRequest("no file content was uploaded".into()));
    }

    let outcome = tokio::task::spawn_blocking(move || -> anyhow::Result<calibre_db::orphans::RelocateOutcome> {
        // `sanitize_file_name` is what stops `../` and friends: the name
        // comes from a client and is about to become a real path.
        let safe = calibre_utils::filenames::sanitize_file_name(&filename);
        if safe.trim().is_empty() {
            anyhow::bail!("{filename:?} is not a usable filename");
        }
        let destination = cache.backend.library_path.join(&safe);
        // Refusing beats overwriting: the existing file may be another
        // book's, and this route's whole job is to stop a file being
        // lost.
        if destination.exists() {
            anyhow::bail!("{safe} already exists in the library; point at it directly instead of uploading over it");
        }
        std::fs::write(&destination, &body)?;

        match calibre_db::orphans::relocate(&cache, book_id, &format, &destination) {
            Ok(outcome) => Ok(outcome),
            Err(e) => {
                // Do not leave a file behind that nothing refers to: the
                // next scan would index it as a book of its own, so a
                // failed repair would quietly invent a duplicate.
                let _ = std::fs::remove_file(&destination);
                Err(e)
            }
        }
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::BadRequest(e.to_string()))?;

    let (code, message) = describe(outcome);
    Ok(Json(json!({"outcome": code, "message": message})))
}

#[derive(Deserialize)]
pub struct ForgetQuery {
    /// Write the book's metadata to `<library>/<name>.opf` before
    /// dropping it. Tags, ratings and reading progress cannot be
    /// re-typed the way a book can be re-downloaded.
    #[serde(default)]
    pub keep_metadata: Option<String>,
}

/// `POST /orphans/forget/{book_id}/{library_id}`.
pub async fn forget(State(state): State<AppState>, Path((book_id, library_id)): Path<(i32, String)>, Query(query): Query<ForgetQuery>) -> Result<Json<Value>, ServerError> {
    let cache = cache_or_404(&state, &library_id)?;

    let saved = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<String>> {
        let destination = match query.keep_metadata.as_deref() {
            Some(name) if !name.trim().is_empty() => {
                let safe = calibre_utils::filenames::sanitize_file_name(name);
                if safe.trim().is_empty() {
                    anyhow::bail!("{name:?} is not a usable filename");
                }
                Some(cache.backend.library_path.join(&safe))
            }
            _ => None,
        };
        calibre_db::orphans::forget(&cache, book_id, destination.as_deref())?;
        Ok(destination.map(|p| p.file_name().unwrap_or_default().to_string_lossy().into_owned()))
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::BadRequest(e.to_string()))?;

    Ok(Json(json!({"forgotten": book_id, "metadata_saved_as": saved})))
}

/// `GET /ignored/{library_id}` -- files removed from the library but kept
/// on disk, which the scanner deliberately will not re-add.
pub async fn ignored(State(state): State<AppState>, Path(library_id): Path<String>) -> Result<Json<Value>, ServerError> {
    let cache = cache_or_404(&state, &library_id)?;
    let files = tokio::task::spawn_blocking(move || cache.ignored().list())
        .await
        .map_err(|e| ServerError::InternalServerError(e.to_string()))?
        .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    Ok(Json(json!(files
        .iter()
        .map(|f| json!({
            "path": f.path,
            // The list is for a person to read, and `scan0042.pdf` alone
            // tells them very little.
            "title": f.title,
            "ignored_at": f.ignored_at,
        }))
        .collect::<Vec<_>>())))
}

#[derive(Deserialize)]
pub struct UnignoreBody {
    pub path: String,
}

/// `POST /ignored/unignore/{library_id}` -- let the scanner pick a file
/// up again. Does not add it: the next scan does that.
pub async fn unignore(State(state): State<AppState>, Path(library_id): Path<String>, Json(body): Json<UnignoreBody>) -> Result<Json<Value>, ServerError> {
    let cache = cache_or_404(&state, &library_id)?;
    tokio::task::spawn_blocking(move || cache.ignored().unignore(&body.path))
        .await
        .map_err(|e| ServerError::InternalServerError(e.to_string()))?
        .map_err(|e| ServerError::InternalServerError(e.to_string()))?;
    Ok(Json(json!({"unignored": true})))
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use calibre_db::cache::Cache;

    /// A router over an already-built library, so a test can arrange the
    /// folder (add a book, then lose its file) before the server sees it.
    fn router_for(cache: Cache) -> axum::Router {
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
        crate::test_router(state)
    }

    /// Builds a library holding one real book file, then loses the file
    /// the way a user would: renamed or moved outside the app.
    fn library_with_an_orphan() -> (tempfile::TempDir, axum::Router, i32) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();

        let book = dir.path().join("Boiler Manual.pdf");
        std::fs::write(&book, b"%PDF-1.4 boiler").unwrap();
        let mut meta = calibre_ebooks::metadata::MetaInformation::default();
        meta.title = "Boiler Manual".to_string();
        meta.authors = vec!["Unknown".to_string()];
        let book_id = cache.register_book_in_place("Boiler Manual.pdf", &meta).unwrap();

        std::fs::remove_file(&book).unwrap();
        cache.orphans().mark(book_id, "PDF", "Boiler Manual.pdf").unwrap();

        let router = router_for(cache);
        (dir, router, book_id)
    }

    async fn get_json(router: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let req = Request::builder().method("GET").uri(uri).body(Body::empty()).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, if bytes.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null) })
    }

    async fn post_body(router: &axum::Router, uri: &str, content_type: &str, body: Vec<u8>) -> (StatusCode, serde_json::Value) {
        let req = Request::builder().method("POST").uri(uri).header("content-type", content_type).body(Body::from(body)).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, if bytes.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null) })
    }

    #[tokio::test]
    async fn an_orphan_is_listed_with_where_it_used_to_be() {
        let (_dir, router, book_id) = library_with_an_orphan();
        let (status, body) = get_json(&router, "/orphans/default").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body.as_array().unwrap().len(), 1, "{body}");
        assert_eq!(body[0]["book_id"], book_id);
        assert_eq!(body[0]["format"], "PDF");
        assert_eq!(body[0]["last_known_path"], "Boiler Manual.pdf");
    }

    /// Resolution one: the file was renamed, and the user points at it.
    #[tokio::test]
    async fn relocating_to_the_renamed_file_clears_the_orphan() {
        let (dir, router, book_id) = library_with_an_orphan();
        // Same bytes, new name -- exactly what a rename outside the app
        // leaves behind.
        std::fs::write(dir.path().join("boiler-manual-v2.pdf"), b"%PDF-1.4 boiler").unwrap();

        let (status, body) = post_body(&router, &format!("/orphans/relocate/{book_id}/PDF/default"), "application/json", br#"{"path": "boiler-manual-v2.pdf"}"#.to_vec()).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["outcome"], "content_matches", "{body}");

        let (_, orphans) = get_json(&router, "/orphans/default").await;
        assert_eq!(orphans.as_array().unwrap().len(), 0, "the orphan should be resolved: {orphans}");
    }

    /// A different file is allowed but must be *said*. Silently accepting
    /// it would hide the case where the user picked the wrong file.
    #[tokio::test]
    async fn relocating_to_different_content_says_so() {
        let (dir, router, book_id) = library_with_an_orphan();
        std::fs::write(dir.path().join("something-else.pdf"), b"%PDF-1.4 a different book entirely").unwrap();

        let (status, body) = post_body(&router, &format!("/orphans/relocate/{book_id}/PDF/default"), "application/json", br#"{"path": "something-else.pdf"}"#.to_vec()).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["outcome"], "content_differs", "{body}");
        assert!(body["message"].as_str().unwrap().contains("not the one that was recorded"), "{body}");
    }

    /// A file outside the library cannot be tracked, so it is refused
    /// with advice rather than silently copied in.
    #[tokio::test]
    async fn relocating_outside_the_library_is_refused() {
        let (_dir, router, book_id) = library_with_an_orphan();
        let (status, body) = post_body(&router, &format!("/orphans/relocate/{book_id}/PDF/default"), "application/json", br#"{"path": "../escaped.pdf"}"#.to_vec()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }

    /// Resolution two: the file really is gone, so the user supplies it.
    #[tokio::test]
    async fn uploading_the_file_back_in_resolves_the_orphan() {
        let (dir, router, book_id) = library_with_an_orphan();

        let (status, body) = post_body(&router, &format!("/orphans/upload/{book_id}/PDF/default/Boiler%20Manual.pdf"), "application/octet-stream", b"%PDF-1.4 boiler".to_vec()).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["outcome"], "content_matches", "{body}");
        assert!(dir.path().join("Boiler Manual.pdf").exists(), "the file should be back in the library");

        let (_, orphans) = get_json(&router, "/orphans/default").await;
        assert_eq!(orphans.as_array().unwrap().len(), 0, "{orphans}");
    }

    /// Uploading over an existing file is refused: it may be another
    /// book's, and this route exists to stop files being lost.
    #[tokio::test]
    async fn uploading_over_an_existing_file_is_refused() {
        let (dir, router, book_id) = library_with_an_orphan();
        std::fs::write(dir.path().join("taken.pdf"), b"%PDF-1.4 somebody elses book").unwrap();

        let (status, body) = post_body(&router, &format!("/orphans/upload/{book_id}/PDF/default/taken.pdf"), "application/octet-stream", b"%PDF-1.4 boiler".to_vec()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(std::fs::read(dir.path().join("taken.pdf")).unwrap(), b"%PDF-1.4 somebody elses book", "the existing file must be untouched");
    }

    #[tokio::test]
    async fn an_empty_upload_is_refused() {
        let (_dir, router, book_id) = library_with_an_orphan();
        let (status, _) = post_body(&router, &format!("/orphans/upload/{book_id}/PDF/default/x.pdf"), "application/octet-stream", Vec::new()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    /// Resolution three: drop the entry.
    #[tokio::test]
    async fn forgetting_an_orphan_removes_it() {
        let (_dir, router, book_id) = library_with_an_orphan();
        let (status, body) = post_body(&router, &format!("/orphans/forget/{book_id}/default"), "application/json", Vec::new()).await;
        assert_eq!(status, StatusCode::OK, "{body}");

        let (_, orphans) = get_json(&router, "/orphans/default").await;
        assert_eq!(orphans.as_array().unwrap().len(), 0, "{orphans}");
    }

    /// Metadata cannot be re-typed the way a book can be re-downloaded,
    /// so it can be saved on the way out.
    #[tokio::test]
    async fn forgetting_can_save_the_metadata_first() {
        let (dir, router, book_id) = library_with_an_orphan();
        let (status, body) = post_body(&router, &format!("/orphans/forget/{book_id}/default?keep_metadata=boiler.opf"), "application/json", Vec::new()).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["metadata_saved_as"], "boiler.opf", "{body}");

        let opf = std::fs::read_to_string(dir.path().join("boiler.opf")).unwrap();
        assert!(opf.contains("Boiler Manual"), "the saved metadata should name the book: {opf}");
    }

    /// The ignore list has to be visible and undoable -- a hidden list of
    /// files the app refuses to show is its own kind of bug.
    #[tokio::test]
    async fn an_ignored_file_is_listed_and_can_be_unignored() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let book = dir.path().join("Boiler Manual.pdf");
        std::fs::write(&book, b"%PDF-1.4 boiler").unwrap();
        let mut meta = calibre_ebooks::metadata::MetaInformation::default();
        meta.title = "Boiler Manual".to_string();
        meta.authors = vec!["Unknown".to_string()];
        let book_id = cache.register_book_in_place("Boiler Manual.pdf", &meta).unwrap();
        // Removed from the library, file kept.
        calibre_db::removal::remove(&cache, book_id, false).unwrap();
        let router = router_for(cache);

        let (status, body) = get_json(&router, "/ignored/default").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body.as_array().unwrap().len(), 1, "{body}");
        assert_eq!(body[0]["path"], "Boiler Manual.pdf");
        assert_eq!(body[0]["title"], "Boiler Manual", "the title is why the list is readable");

        let (status, _) = post_body(&router, "/ignored/unignore/default", "application/json", br#"{"path": "Boiler Manual.pdf"}"#.to_vec()).await;
        assert_eq!(status, StatusCode::OK);

        let (_, after) = get_json(&router, "/ignored/default").await;
        assert_eq!(after.as_array().unwrap().len(), 0, "{after}");
    }
}
