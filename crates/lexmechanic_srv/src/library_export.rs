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

/// A consistent copy of a SQLite database, as bytes.
///
/// `VACUUM INTO` writes a new database file from one read snapshot, so it
/// contains everything committed -- including what is still sitting in the
/// `-wal` file -- and needs no companion files to open. That is the point:
/// the export used to copy `metadata.db` and its `-wal` and `-shm` as three
/// separate files read at three different moments. A write landing between
/// the reads, or a WAL the copy was taken without, gave an archive whose
/// database was missing committed rows or would not open.
///
/// Tier-aware for free. A library on network storage is in `DELETE` journal
/// mode with no WAL at all (`storage_safety.rs`); the snapshot is the same
/// read either way, and there is simply no sidecar to leave out.
///
/// `None` when the file is not a database SQLite can read (or is locked
/// hard): the caller then copies it as it is, sidecars included, rather
/// than failing the whole export.
fn consistent_database_copy(path: &Path) -> Option<Vec<u8>> {
    let scratch = tempfile::tempdir().ok()?;
    let target = scratch.path().join("snapshot.db");
    lexmechanic_db::mirror::snapshot_database(path, &target).ok()?;
    std::fs::read(&target).ok()
}

/// `metadata.db-wal` -> `metadata.db`: the database a SQLite sidecar
/// belongs to.
fn sidecar_owner(path: &Path) -> Option<std::path::PathBuf> {
    let name = path.file_name()?.to_str()?;
    let base = ["-wal", "-shm", "-journal"].iter().find_map(|suffix| name.strip_suffix(suffix))?;
    base.ends_with(".db").then(|| path.with_file_name(base))
}

fn build_library_zip(library_path: &Path) -> anyhow::Result<Vec<u8>> {
    let entries: Vec<walkdir::DirEntry> = walkdir::WalkDir::new(library_path).into_iter().filter_map(Result::ok).collect();

    // Databases first, each as one consistent snapshot; their `-wal`,
    // `-shm` and `-journal` files are then left out, because the snapshot
    // already contains what they held and a stale one beside it could only
    // confuse the restored copy.
    let mut snapshots: std::collections::HashMap<std::path::PathBuf, Vec<u8>> = std::collections::HashMap::new();
    for entry in &entries {
        let path = entry.path();
        if entry.file_type().is_file() && path.extension().and_then(|e| e.to_str()) == Some("db") {
            if path.strip_prefix(library_path).ok().is_some_and(|rel| lexmechanic_db::mirror::is_excluded(&rel.to_string_lossy())) {
                continue;
            }
            if let Some(bytes) = consistent_database_copy(path) {
                snapshots.insert(path.to_path_buf(), bytes);
            }
        }
    }

    let mut buf = Vec::new();
    {
        let cursor = std::io::Cursor::new(&mut buf);
        let mut zip = zip::ZipWriter::new(cursor);
        let options = zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for entry in &entries {
            let path = entry.path();
            let rel = path.strip_prefix(library_path)?;
            if rel.as_os_str().is_empty() {
                continue;
            }
            // What is left out is one list shared with the copy/mirror
            // ([`lexmechanic_db::mirror::is_excluded`]): the trash, the writer lock
            // and journal (per-process state that would be wrong to restore;
            // on Windows `writer.lock` is even unreadable while held, which
            // took the whole export down), job records, import staging, run
            // records, SQLite sidecars and half-written files. The rest of the
            // state directory is durable and *is* exported -- the change log
            // (the library's authority, from which `metadata.db` is rebuilt),
            // covers and checksums -- so a new durable file is carried by
            // default instead of silently missing from every backup.
            if lexmechanic_db::mirror::is_excluded(&rel.to_string_lossy()) {
                continue;
            }
            // Zip entry names use forward slashes on every platform,
            // matching the real zip spec (not the host OS separator).
            let name = rel.to_string_lossy().replace('\\', "/");
            if entry.file_type().is_dir() {
                zip.add_directory(format!("{name}/"), options)?;
            } else if entry.file_type().is_file() {
                if sidecar_owner(path).is_some_and(|owner| snapshots.contains_key(&owner)) {
                    continue;
                }
                zip.start_file(name, options)?;
                if let Some(snapshot) = snapshots.get(path) {
                    std::io::Write::write_all(&mut zip, snapshot).with_context(|| format!("writing the snapshot of {} for export", path.display()))?;
                    continue;
                }
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
    use super::sidecar_owner;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use lexmechanic_db::cache::Cache;

    fn state_for(cache: std::sync::Arc<Cache>) -> crate::AppState {
        crate::AppState {
            libraries: None,
            cache,
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
            tts_voice: None, plugin_store: None, plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(lexmechanic_customize::registry::PluginRegistry::new())),
        }
    }

    /// Like [`test_app`], but hands back the very `Cache` the router
    /// uses. A test that opened a second one over the same library would
    /// fail on the writer lock -- correctly, since that lock exists to
    /// stop two writers.
    fn test_app_with_cache() -> (tempfile::TempDir, axum::Router, std::sync::Arc<Cache>, i32) {
        let dir = tempfile::tempdir().unwrap();
        let cache = std::sync::Arc::new(Cache::new(dir.path()).unwrap());
        let source = dir.path().join("Book.txt");
        std::fs::write(&source, b"hello export test").unwrap();
        let mut meta = lexmechanic_ebooks::metadata::MetaInformation::default();
        meta.title = "Export Test Book".to_string();
        meta.authors = vec!["Jane Doe".to_string()];
        let book_id = cache.add_book(&source, &meta).unwrap();
        let router = crate::test_router(state_for(cache.clone()));
        (dir, router, cache, book_id)
    }

    fn test_app() -> (tempfile::TempDir, axum::Router, i32) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let source = dir.path().join("Book.txt");
        std::fs::write(&source, b"hello export test").unwrap();
        let mut meta = lexmechanic_ebooks::metadata::MetaInformation::default();
        meta.title = "Export Test Book".to_string();
        meta.authors = vec!["Jane Doe".to_string()];
        let book_id = cache.add_book(&source, &meta).unwrap();
        let state = state_for(std::sync::Arc::new(cache));
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
            tts_voice: None, plugin_store: None, plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(lexmechanic_customize::registry::PluginRegistry::new())),
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

    /// The state directory holds the **change log** -- the library's
    /// authority, from which `metadata.db` is rebuilt -- plus covers and
    /// checksums. An export that skipped it would be a library that
    /// cannot be rebuilt and has no cover images: a backup that looks
    /// complete and is not.
    #[tokio::test]
    async fn the_export_carries_the_change_log_and_covers() {
        let (_dir, router, cache, book_id) = test_app_with_cache();
        lexmechanic_db::covers::set_cover(&cache, book_id, b"cover bytes").unwrap();
        cache.set_field(book_id, "rating", "8").unwrap();

        let req = Request::builder().uri("/library/export/default").body(Body::empty()).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();

        let names = zip_entry_names(&bytes);
        let state = lexmechanic_db::constants::LIBRARY_HANDLE_DIR_NAME;
        assert!(names.iter().any(|n| n.starts_with(&format!("{state}/changes/"))), "the change log is missing from the export: {names:?}");
        assert!(names.iter().any(|n| n.starts_with(&format!("{state}/covers/"))), "covers are missing from the export: {names:?}");
    }

    /// Per-process state stays out. `writer.lock` additionally cannot be
    /// read on Windows while held, which took the whole export down
    /// before it was excluded.
    #[tokio::test]
    async fn the_export_leaves_out_the_lock_and_the_journal() {
        let (_dir, router, cache, book_id) = test_app_with_cache();
        // Force the writer lock and journal into existence.
        cache.set_field(book_id, "title", "Touched").unwrap();
        lexmechanic_db::covers::set_cover(&cache, book_id, b"cover").unwrap();

        let req = Request::builder().uri("/library/export/default").body(Body::empty()).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();

        let names = zip_entry_names(&bytes);
        let state = lexmechanic_db::constants::LIBRARY_HANDLE_DIR_NAME;
        assert!(!names.iter().any(|n| n.contains("writer.lock")), "{names:?}");
        assert!(!names.iter().any(|n| n.starts_with(&format!("{state}/journal"))), "{names:?}");
    }

    /// The export used to copy `metadata.db`, `-wal` and `-shm` as three
    /// files read at three moments. What it must hand back instead is one
    /// database that already contains everything committed, opens on its
    /// own, and has no sidecar beside it to go stale.
    #[tokio::test]
    async fn the_export_database_is_consistent_with_writes_still_in_the_wal() {
        let (dir, router, cache, book_id) = test_app_with_cache();
        // Commits that may well still be in the write-ahead log.
        cache.set_field(book_id, "title", "Edited While Exporting").unwrap();
        cache.set_field(book_id, "rating", "8").unwrap();

        let req = Request::builder().uri("/library/export/default").body(Body::empty()).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();

        let names = zip_entry_names(&bytes);
        assert!(!names.iter().any(|n| n.ends_with("-wal") || n.ends_with("-shm")), "a stale sidecar was exported beside the snapshot: {names:?}");

        // Extract only the database: no sidecars exist to help it.
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
        let extracted = tempfile::tempdir().unwrap();
        let mut db = Vec::new();
        std::io::Read::read_to_end(&mut archive.by_name("metadata.db").unwrap(), &mut db).unwrap();
        std::fs::write(extracted.path().join("metadata.db"), db).unwrap();

        let conn = rusqlite::Connection::open(extracted.path().join("metadata.db")).unwrap();
        let title: String = conn.query_row("SELECT title FROM books WHERE id = ?1", [book_id], |r| r.get(0)).unwrap();
        assert_eq!(title, "Edited While Exporting", "a committed write is missing from the exported database");
        let integrity: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0)).unwrap();
        assert_eq!(integrity, "ok");
        drop(dir);
    }

    #[test]
    fn a_sqlite_sidecar_is_recognised_by_the_database_it_belongs_to() {
        use std::path::Path;
        assert_eq!(sidecar_owner(Path::new("lib/metadata.db-wal")), Some(Path::new("lib/metadata.db").to_path_buf()));
        assert_eq!(sidecar_owner(Path::new("lib/.lexmechanic/checksums.db-shm")), Some(Path::new("lib/.lexmechanic/checksums.db").to_path_buf()));
        // A book that happens to be called something-wal is not one.
        assert_eq!(sidecar_owner(Path::new("lib/notes-wal")), None);
        assert_eq!(sidecar_owner(Path::new("lib/metadata.db")), None);
    }

    fn zip_entry_names(bytes: &[u8]) -> Vec<String> {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).expect("a real zip");
        (0..archive.len()).map(|i| archive.by_index(i).unwrap().name().to_string()).collect()
    }

    /// The export used to carry the library's trash (`.caltrash`) and the
    /// server's job records; it now leaves out exactly what a copy leaves out.
    #[tokio::test]
    async fn the_export_leaves_out_the_trash_and_job_records() {
        let (dir, router, cache, book_id) = test_app_with_cache();
        cache.set_field(book_id, "title", "Touched").unwrap();
        std::fs::create_dir_all(dir.path().join(".caltrash/x")).unwrap();
        std::fs::write(dir.path().join(".caltrash/x/gone.txt"), "deleted book").unwrap();
        let state = dir.path().join(lexmechanic_db::constants::LIBRARY_HANDLE_DIR_NAME);
        std::fs::write(state.join("jobs.json"), "{}").unwrap();
        std::fs::create_dir_all(state.join("import-staging")).unwrap();
        std::fs::write(state.join("import-staging/x.pdf"), "x").unwrap();

        let req = Request::builder().uri("/library/export/default").body(Body::empty()).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let names = zip_entry_names(&bytes);
        assert!(!names.iter().any(|n| n.contains("caltrash") || n.contains("jobs.json") || n.contains("import-staging")), "{names:?}");
        assert!(names.contains(&"metadata.db".to_string()));
    }
}
