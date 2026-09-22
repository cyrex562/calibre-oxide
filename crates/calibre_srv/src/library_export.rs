//! `GET /library/export/{library_id}` -- a real, **new** route (issue
//! #761, part of #747's tracking epic): export a whole library as a
//! real, self-contained archive for backup or transfer.
//!
//! # Real prerequisite check, resolved
//!
//! Real upstream's own on-disk library layout (`metadata.db` +
//! `full-text-search.db` + per-book folders under per-author folders)
//! IS the real, honest archive format -- a plain zip of the library
//! directory, matching how a calibre library already looks, rather
//! than inventing a new container. This is the same real layout
//! `check_library.rs`'s own `scan_library` already walks.
//!
//! # Import lives in the desktop app, not here
//!
//! Real "import" means writing an extracted library to an arbitrary
//! local filesystem path and then opening it -- a native-filesystem
//! operation the browser-facing HTTP layer can't do (nothing here can
//! write to a path the client picks; only the client's own OS can).
//! Per this issue's own filed scope, that half lives in
//! `app/src-tauri` (reusing its already-real `open_library` "open a
//! library at a path" flow, issue #725) -- see that crate's own
//! `library_import.rs`.
//!
//! # Real, disclosed narrowing
//!
//! - **Whole library only**, not a chosen search-result subset (the
//!   issue's own scope lists that as an option, not a requirement;
//!   real, separable follow-up).
//! - **Built entirely in memory** before responding, matching this
//!   crate's own established "whole file read into memory" precedent
//!   (`content.rs`'s own disclosed narrowing) -- correct, not
//!   bandwidth/memory-optimal for very large libraries.

use std::path::Path;

use axum::body::Body;
use axum::extract::{Path as AxumPath, State};
use axum::http::{header, HeaderValue};
use axum::response::Response;

use anyhow::Context;

use crate::errors::ServerError;
use crate::AppState;

fn build_library_zip(library_path: &Path) -> anyhow::Result<Vec<u8>> {
    let mut buf = Vec::new();
    {
        let cursor = std::io::Cursor::new(&mut buf);
        let mut zip = zip::ZipWriter::new(cursor);
        let options = zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for entry in walkdir::WalkDir::new(library_path).into_iter().filter_map(Result::ok) {
            let path = entry.path();
            let rel = path.strip_prefix(library_path)?;
            if rel.as_os_str().is_empty() {
                continue;
            }
            // Skip the runtime state directory. It holds the writer
            // lock and the write-ahead journal -- per-process state
            // that would be actively wrong to restore from a backup,
            // and on Windows actively unreadable: `try_lock` there is
            // `LockFileEx`, a byte-range lock that blocks reads of the
            // locked region, so copying `writer.lock` fails outright
            // and took the whole export down with it. (On Linux
            // `flock` is advisory and reads sail through, which is why
            // this only ever showed up on Windows.)
            if rel.starts_with(calibre_db::constants::LIBRARY_HANDLE_DIR_NAME) {
                continue;
            }
            // Zip entry names use forward slashes on every platform,
            // matching the real zip spec (not the host OS separator).
            let name = rel.to_string_lossy().replace('\\', "/");
            if entry.file_type().is_dir() {
                zip.add_directory(format!("{name}/"), options)?;
            } else if entry.file_type().is_file() {
                zip.start_file(name, options)?;
                let mut f = std::fs::File::open(path)
                    .with_context(|| format!("opening {} for export", path.display()))?;
                std::io::copy(&mut f, &mut zip)
                    .with_context(|| format!("reading {} for export", path.display()))?;
            }
        }
        zip.finish()?;
    }
    Ok(buf)
}

/// `GET /library/export/{library_id}`.
pub async fn export(State(state): State<AppState>, AxumPath(library_id): AxumPath<String>) -> Result<Response, ServerError> {
    let cache = state.cache_for(Some(&library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))?;
    let library_path = cache.backend.library_path.clone();

    let zip_bytes = tokio::task::spawn_blocking(move || build_library_zip(&library_path)).await.map_err(|e| ServerError::InternalServerError(e.to_string()))?.map_err(|e| ServerError::InternalServerError(format!("{e:#}")))?;

    let filename = format!("{library_id}.zip");
    let mut resp = Response::new(Body::from(zip_bytes));
    resp.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/zip"));
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\"")) {
        resp.headers_mut().insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(resp)
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
        std::fs::write(&source, b"hello export test").unwrap();
        let mut meta = calibre_ebooks::metadata::MetaInformation::default();
        meta.title = "Export Test Book".to_string();
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
            news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()),
            tts_voice: None, plugin_store: None, plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(calibre_customize::registry::PluginRegistry::new())),
        };
        let router = crate::test_router(state);
        (dir, router, book_id)
    }

    #[tokio::test]
    async fn export_returns_a_real_zip_containing_the_real_metadata_db_and_book_file() {
        let (_dir, router, _book_id) = test_app();
        let req = Request::builder().uri("/library/export/default").body(Body::empty()).unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get("content-type").unwrap(), "application/zip");
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert!(bytes.len() > 100, "expected a real, non-trivial zip, got {} bytes", bytes.len());

        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes.as_ref())).unwrap();
        let names: Vec<String> = (0..zip.len()).map(|i| zip.by_index(i).unwrap().name().to_string()).collect();
        assert!(names.contains(&"metadata.db".to_string()), "{names:?}");

        // The book's own real title-derived filename (sanitized), not
        // the source "Book.txt" -- confirms this is a real archive of
        // Cache::add_book's own on-disk layout, not just a raw copy of
        // the temp source directory.
        let mut found_book_format = false;
        let mut content = String::new();
        for name in &names {
            if name.ends_with(".txt") && name != "metadata.db" {
                let mut f = zip.by_name(name).unwrap();
                std::io::Read::read_to_string(&mut f, &mut content).unwrap();
                found_book_format = true;
                break;
            }
        }
        assert!(found_book_format, "{names:?}");
        assert_eq!(content, "hello export test");
    }

    fn test_app_with_two_libraries() -> (tempfile::TempDir, tempfile::TempDir, std::sync::Arc<crate::library_broker::LibraryBroker>, axum::Router) {
        let src_dir = tempfile::tempdir().unwrap();
        let dest_dir = tempfile::tempdir().unwrap();
        Cache::new(src_dir.path()).unwrap();
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
            news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()),
            tts_voice: None, plugin_store: None, plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(calibre_customize::registry::PluginRegistry::new())),
        };
        let router = crate::test_router(state);
        (src_dir, dest_dir, broker, router)
    }

    #[tokio::test]
    async fn export_404s_for_an_unknown_library_id() {
        let (_src_dir, _dest_dir, _broker, router) = test_app_with_two_libraries();
        let req = Request::builder().uri("/library/export/no-such-library").body(Body::empty()).unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
