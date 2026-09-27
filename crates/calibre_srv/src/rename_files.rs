//! `POST /rename-files/{library_id}` -- renaming a book's files on
//! disk, as an operation in its own right (issue #885).
//!
//! # Why this is separate from retitling
//!
//! A book's title and the name of the file holding it were one thing.
//! `Cache::add_format` names a file after the title at the moment it
//! is added, and the only code that ever renamed a file afterwards was
//! `Cache::update_book_metadata` -- so the only way to change a
//! filename was to change the title, and changing the title always
//! changed the filename. Neither half is what somebody organising a
//! folder of scanned PDFs actually wants.
//!
//! This route changes filenames and nothing else. The title, the
//! author and the `<author>/<title>` folder are metadata-derived and
//! have their own operations; none of them move here.
//!
//! # Why the template dialect is save-to-disk's
//!
//! Names come from a template, evaluated by
//! `calibre_utils::formatter::string_format::evaluate_template` --
//! the same function and therefore the same dialect
//! `save_to_disk.rs` uses, so `{title} - {authors}` means here what it
//! means there. Deliberately not `template_tester.rs`'s route, which
//! accepts Template Program Mode only (`field('title')`) and would
//! have made this app's two template boxes speak different languages.
//!
//! A template with no substitutions in it is just a literal, which is
//! what makes "rename this one book to this one name" the same
//! request as "rename these forty books from their metadata" rather
//! than a second endpoint.
//!
//! # Why preview and apply are one route
//!
//! `dry_run` selects between them. Renaming cannot be undone, so a
//! preview is not a nicety -- and a preview computed by different code
//! from the rename is a preview that can lie. Sharing the route means
//! the proposed names in the preview are produced by exactly the code
//! that will use them, including collision resolution.

use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use calibre_db::cache::Cache;
use calibre_db::formatter_functions::{CacheCatalog, CacheFunctions, CacheValueSource};
use calibre_utils::filenames::sanitize_file_name;
use calibre_utils::formatter::string_format;

use crate::errors::ServerError;
use crate::web_socket::{self, ChangeEvent};
use crate::AppState;

/// Books one request may rename. Matches `template_tester`'s own bulk
/// cap -- the work per book is a template evaluation plus a handful of
/// file renames, and a request that walks a whole library should be
/// several requests.
const MAX_BOOKS: usize = 500;

#[derive(Debug, Deserialize)]
pub struct RenameFilesBody {
    book_ids: Vec<i32>,
    /// A calibre template in the `{field}` dialect, or a plain string
    /// to use literally.
    template: String,
    /// Compute the new names and report them without touching a
    /// single file. The UI previews first, always.
    #[serde(default)]
    dry_run: bool,
}

/// What one book's rename would do, or did.
struct Proposal {
    book_id: i32,
    title: String,
    current: String,
    proposed: Result<String, String>,
}

/// Evaluates `template` for one book and resolves it to a usable,
/// unclaimed filename stem.
///
/// `claimed` carries the names already handed out in this batch.
/// Two books can share a folder (same author, same title), and a
/// template keyed on anything they have in common produces the same
/// name for both -- so the batch has to remember what it has already
/// promised, not just what is on disk. Without it, the second book's
/// rename would fail at write time against a file the first book had
/// only just created.
fn propose(cache: &Cache, book_id: i32, template: &str, claimed: &mut std::collections::HashSet<(String, String)>) -> Proposal {
    let title = cache.field_for(book_id, "title").ok().flatten().unwrap_or_default();
    let current = cache.format_file_stem(book_id).ok().flatten().unwrap_or_default();

    let proposed = (|| -> Result<String, String> {
        let value_source = CacheValueSource::new(cache, book_id).map_err(|e| e.to_string())?;
        let functions = CacheFunctions::new(cache, book_id);
        let rendered = string_format::evaluate_template(template, &value_source, &CacheCatalog, &functions)?;

        // A filename, not a path: a template written for save-to-disk
        // has `/` in it, and the last component is the part that names
        // the file. Taking it rather than rejecting the template means
        // somebody can paste the template they already use.
        let last = rendered.rsplit('/').next().unwrap_or(&rendered).trim();
        let stem = sanitize_file_name(last);
        if stem.trim().is_empty() {
            return Err("the template produced an empty filename".to_string());
        }

        let folder = cache.field_for(book_id, "path").ok().flatten().unwrap_or_default();
        let mut stem = cache.available_format_stem(book_id, &stem).map_err(|e| e.to_string())?;
        let mut suffix = 1;
        while !claimed.insert((folder.clone(), stem.clone())) {
            stem = format!("{} ({suffix})", sanitize_file_name(last));
            suffix += 1;
            if suffix > 1000 {
                return Err("too many books in one folder want this name".to_string());
            }
        }
        Ok(stem)
    })();

    Proposal { book_id, title, current, proposed }
}

/// `POST /rename-files/{library_id}`.
pub async fn rename_files(State(state): State<AppState>, Path(library_id): Path<String>, Json(body): Json<RenameFilesBody>) -> Result<Json<Value>, ServerError> {
    let cache = state.cache_for(Some(&library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))?;
    if body.book_ids.len() > MAX_BOOKS {
        return Err(ServerError::BadRequest(format!("at most {MAX_BOOKS} books can be renamed in one request (got {})", body.book_ids.len())));
    }
    if body.template.trim().is_empty() {
        return Err(ServerError::BadRequest("a name or template is required".to_string()));
    }

    let dry_run = body.dry_run;
    let (results, renamed_ids) = tokio::task::spawn_blocking(move || {
        let mut claimed = std::collections::HashSet::new();
        let mut results = Vec::new();
        let mut renamed_ids = Vec::new();

        for book_id in &body.book_ids {
            let proposal = propose(&cache, *book_id, &body.template, &mut claimed);
            let mut entry = json!({
                "book_id": proposal.book_id,
                "title": proposal.title,
                "current": proposal.current,
            });

            match &proposal.proposed {
                Err(error) => {
                    entry["error"] = json!(error);
                    entry["changed"] = json!(false);
                }
                Ok(proposed) => {
                    let changed = *proposed != proposal.current;
                    entry["proposed"] = json!(proposed);
                    entry["changed"] = json!(changed);

                    if !dry_run && changed {
                        // One book failing does not abandon the rest:
                        // a batch that stopped halfway would leave the
                        // user guessing which half had moved.
                        match cache.rename_format_files(*book_id, proposed) {
                            Ok(_) => renamed_ids.push(*book_id),
                            Err(e) => {
                                entry["error"] = json!(e.to_string());
                                entry["changed"] = json!(false);
                            }
                        }
                    }
                }
            }
            results.push(entry);
        }

        (results, renamed_ids)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    if !renamed_ids.is_empty() {
        // The `fmt_*` paths in every cached book row just changed, so
        // any open view holding one is now pointing at a file that has
        // moved.
        web_socket::publish(&state, ChangeEvent::MetadataChanged { book_ids: renamed_ids.clone() });
    }

    Ok(Json(json!({ "dry_run": dry_run, "renamed": renamed_ids.len(), "results": results })))
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use calibre_db::cache::Cache;
    use tower::ServiceExt;

    fn add_pdf(dir: &std::path::Path, cache: &Cache, title: &str, author: &str, stem: &str) -> i32 {
        let source = dir.join(format!("{stem}.pdf"));
        std::fs::write(&source, b"%PDF-1.4 pretend").unwrap();
        let mut meta = calibre_ebooks::metadata::MetaInformation::default();
        meta.title = title.to_string();
        meta.authors = vec![author.to_string()];
        cache.add_book(&source, &meta).unwrap()
    }

    /// A router over a `Cache` the test keeps its own handle on, so it
    /// can read filenames back after a rename.
    fn app(dir: &std::path::Path) -> (std::sync::Arc<Cache>, axum::Router) {
        let cache = std::sync::Arc::new(Cache::new(dir).unwrap());
        let state = crate::AppState {
            libraries: None,
            cache: cache.clone(),
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
            plugin_store: None,
            plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(calibre_customize::registry::PluginRegistry::new())),
        };
        (cache, crate::test_router(state))
    }

    async fn post(router: &axum::Router, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let req = Request::builder().method("POST").uri("/rename-files/default").header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }

    #[tokio::test]
    async fn a_dry_run_reports_the_new_name_without_moving_anything() {
        let dir = tempfile::tempdir().unwrap();
        let (cache, router) = app(dir.path());
        let id = add_pdf(dir.path(), &cache, "A Title", "An Author", "A Title");

        let (status, body) = post(&router, serde_json::json!({ "book_ids": [id], "template": "Renamed", "dry_run": true })).await;
        assert_eq!(status, StatusCode::OK, "got: {body}");
        assert_eq!(body["results"][0]["current"], "A Title");
        assert_eq!(body["results"][0]["proposed"], "Renamed");
        assert_eq!(body["results"][0]["changed"], true);
        assert_eq!(body["renamed"], 0);

        // Nothing moved. This is the property that makes the preview
        // worth having at all.
        assert_eq!(cache.format_file_stem(id).unwrap().as_deref(), Some("A Title"));
    }

    #[tokio::test]
    async fn applying_renames_the_file_and_leaves_the_title_alone() {
        let dir = tempfile::tempdir().unwrap();
        let (cache, router) = app(dir.path());
        let id = add_pdf(dir.path(), &cache, "A Title", "An Author", "A Title");

        let (status, body) = post(&router, serde_json::json!({ "book_ids": [id], "template": "Renamed" })).await;
        assert_eq!(status, StatusCode::OK, "got: {body}");
        assert_eq!(body["renamed"], 1);

        assert_eq!(cache.format_file_stem(id).unwrap().as_deref(), Some("Renamed"));
        assert_eq!(cache.field_for(id, "title").unwrap().as_deref(), Some("A Title"));
        assert!(dir.path().join("Renamed.pdf").exists());
    }

    #[tokio::test]
    async fn a_template_is_evaluated_against_the_books_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let (cache, router) = app(dir.path());
        let id = add_pdf(dir.path(), &cache, "Dune", "Frank Herbert", "scan0001");

        let (status, body) = post(&router, serde_json::json!({ "book_ids": [id], "template": "{title} - {authors}" })).await;
        assert_eq!(status, StatusCode::OK, "got: {body}");
        assert_eq!(body["results"][0]["proposed"], "Dune - Frank Herbert");
        assert!(dir.path().join("Dune - Frank Herbert.pdf").exists());
    }

    #[tokio::test]
    async fn a_save_to_disk_style_path_template_names_the_file_after_its_last_component() {
        let dir = tempfile::tempdir().unwrap();
        let (cache, router) = app(dir.path());
        let id = add_pdf(dir.path(), &cache, "Dune", "Frank Herbert", "scan0001");

        // The template people already have saved for save-to-disk.
        let (status, body) = post(&router, serde_json::json!({ "book_ids": [id], "template": "{author_sort}/{title}/{title} - {authors}", "dry_run": true })).await;
        assert_eq!(status, StatusCode::OK, "got: {body}");
        assert_eq!(body["results"][0]["proposed"], "Dune - Frank Herbert");
    }

    #[tokio::test]
    async fn two_books_in_one_folder_wanting_the_same_name_are_disambiguated() {
        let dir = tempfile::tempdir().unwrap();
        let (cache, router) = app(dir.path());
        // Same author and title, so one folder -- and a template keyed
        // on the title gives both the same answer.
        let first = add_pdf(dir.path(), &cache, "Same", "An Author", "one");
        let second = add_pdf(dir.path(), &cache, "Same", "An Author", "two");

        let (status, body) = post(&router, serde_json::json!({ "book_ids": [first, second], "template": "{title}" })).await;
        assert_eq!(status, StatusCode::OK, "got: {body}");

        let names: Vec<&str> = body["results"].as_array().unwrap().iter().map(|r| r["proposed"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["Same", "Same (1)"], "one book silently took the other's name");
        assert_eq!(cache.format_file_stem(first).unwrap().as_deref(), Some("Same"));
        assert_eq!(cache.format_file_stem(second).unwrap().as_deref(), Some("Same (1)"));
        // Both files really are still there.
        assert!(dir.path().join("Same.pdf").exists());
        assert!(dir.path().join("Same (1).pdf").exists());
    }

    #[tokio::test]
    async fn a_template_that_produces_nothing_is_reported_per_book_not_as_a_failure() {
        let dir = tempfile::tempdir().unwrap();
        let (cache, router) = app(dir.path());
        let id = add_pdf(dir.path(), &cache, "A Title", "An Author", "A Title");

        // `series` is empty for this book, so the template renders to
        // nothing at all.
        let (status, body) = post(&router, serde_json::json!({ "book_ids": [id], "template": "{series}" })).await;
        assert_eq!(status, StatusCode::OK, "got: {body}");
        assert!(body["results"][0]["error"].as_str().unwrap().contains("empty"), "got: {body}");
        assert_eq!(body["renamed"], 0);
        assert_eq!(cache.format_file_stem(id).unwrap().as_deref(), Some("A Title"));
    }

    #[tokio::test]
    async fn one_book_failing_does_not_abandon_the_others() {
        let dir = tempfile::tempdir().unwrap();
        let (cache, router) = app(dir.path());
        let good = add_pdf(dir.path(), &cache, "Has A Series", "An Author", "Has A Series");
        cache.set_field(good, "series", "A Series").unwrap();
        let bad = add_pdf(dir.path(), &cache, "No Series", "An Author", "No Series");

        let (status, body) = post(&router, serde_json::json!({ "book_ids": [bad, good], "template": "{series}" })).await;
        assert_eq!(status, StatusCode::OK, "got: {body}");
        assert_eq!(body["renamed"], 1);
        assert!(body["results"][0]["error"].is_string());
        assert_eq!(cache.format_file_stem(good).unwrap().as_deref(), Some("A Series"));
    }

    #[tokio::test]
    async fn renaming_to_the_current_name_is_reported_as_no_change() {
        let dir = tempfile::tempdir().unwrap();
        let (cache, router) = app(dir.path());
        let id = add_pdf(dir.path(), &cache, "A Title", "An Author", "A Title");

        let (status, body) = post(&router, serde_json::json!({ "book_ids": [id], "template": "{title}", "dry_run": true })).await;
        assert_eq!(status, StatusCode::OK, "got: {body}");
        assert_eq!(body["results"][0]["changed"], false);
        // Not `A Title (1)`: a book's own filename is not a collision
        // with itself.
        assert_eq!(body["results"][0]["proposed"], "A Title");
    }

    #[tokio::test]
    async fn an_empty_template_is_rejected_outright() {
        let dir = tempfile::tempdir().unwrap();
        let (cache, router) = app(dir.path());
        let id = add_pdf(dir.path(), &cache, "A Title", "An Author", "A Title");

        let (status, _) = post(&router, serde_json::json!({ "book_ids": [id], "template": "   " })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_renamed_book_still_resolves_to_a_file_that_exists() {
        let dir = tempfile::tempdir().unwrap();
        let (cache, router) = app(dir.path());
        let id = add_pdf(dir.path(), &cache, "A Title", "An Author", "A Title");

        post(&router, serde_json::json!({ "book_ids": [id], "template": "Something Else" })).await;

        // The invariant the whole issue is about: `data.name` and the
        // file on disk in step, so every route that serves a book
        // still finds it.
        let row = cache.get_data_as_dict(None, false, None, false).unwrap()[0].clone();
        let path = row["fmt_pdf"].as_str().expect("the format must still resolve after a rename");
        assert!(std::path::Path::new(path).exists(), "{path} does not exist");
    }
}
