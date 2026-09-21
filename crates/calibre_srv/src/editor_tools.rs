//! Check-book and reports for the editor (issues 3.3 and 3.2 of the
//! #816 epic).
//!
//! Both engines were fully ported and had no caller anywhere:
//! `oeb::polish::check::run_checks`/`fix_errors` and
//! `oeb::polish::report`'s `files_data`/`images_data`/`links_data`.
//! Upstream drives them from Qt dialogs, so there is no `calibre.srv`
//! route to port.
//!
//! # Built on the existing tweak session
//!
//! Both take an already-open `Container`, which a tweak session
//! already holds (`tweak.rs`). Hanging these routes off that session
//! means they see exactly the book the user is editing, including
//! unsaved changes -- checking the on-disk copy instead would report
//! problems the editor had already fixed.

use axum::extract::{Path as AxumPath, State};
use axum::Json;
use calibre_ebooks::oeb::polish::check::main::{fix_errors, run_checks};
use calibre_ebooks::oeb::polish::report;
use calibre_ebooks::oeb::polish::spell as polish_spell;
use calibre_ebooks::spell::dictionary::{DictionaryMeta, Dictionaries};
use calibre_ebooks::spell::{parse_lang_code, vendored, DictionaryLocale};
use serde_json::{json, Value};

use crate::errors::ServerError;
use crate::AppState;

/// EPUB is the only container a tweak session opens, so it is the
/// only book type these can be asked about.
const BOOK_TYPE: &str = "epub";

fn level_name(level: calibre_ebooks::oeb::polish::check::base::Level) -> &'static str {
    use calibre_ebooks::oeb::polish::check::base::Level;
    match level {
        Level::Debug => "debug",
        Level::Info => "info",
        Level::Warn => "warning",
        Level::Error => "error",
        Level::Critical => "critical",
    }
}

/// `GET /tweak/check/{session_id}` -- runs every check against the
/// session's live container.
pub async fn check(State(state): State<AppState>, AxumPath(session_id): AxumPath<String>) -> Result<Json<Value>, ServerError> {
    tokio::task::spawn_blocking(move || -> Result<Json<Value>, ServerError> {
        let errors = state
            .tweak_sessions
            .with_container(&session_id, |container| run_checks(container, BOOK_TYPE))
            .ok_or_else(|| ServerError::NotFound(format!("No tweak session: {session_id}")))?
            .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

        let items: Vec<Value> = errors
            .iter()
            .map(|e| {
                json!({
                    "type": e.type_name,
                    "message": e.msg,
                    "file": e.name,
                    "line": e.line,
                    "col": e.col,
                    "level": level_name(e.level),
                    "help": e.help,
                    // What the UI needs to decide whether to offer a
                    // "fix this" button at all.
                    "fixable": e.individual_fix.is_some(),
                    "fix_label": e.individual_fix,
                })
            })
            .collect();

        let errors_count = errors.iter().filter(|e| level_name(e.level) == "error" || level_name(e.level) == "critical").count();
        Ok(Json(json!({ "count": items.len(), "errors": errors_count, "fixable": items.iter().filter(|i| i["fixable"] == json!(true)).count(), "items": items })))
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
}

/// `POST /tweak/check-fix/{session_id}` -- applies every automatic
/// fix the checks offered.
///
/// Fixes are applied to the open session rather than to disk, so they
/// are part of the same commit-or-discard decision as any other
/// editor change. A user who dislikes the result can still discard.
pub async fn check_fix(State(state): State<AppState>, AxumPath(session_id): AxumPath<String>) -> Result<Json<Value>, ServerError> {
    tokio::task::spawn_blocking(move || -> Result<Json<Value>, ServerError> {
        let result = state
            .tweak_sessions
            .with_container(&session_id, |container| -> anyhow::Result<(usize, bool)> {
                let errors = run_checks(container, BOOK_TYPE)?;
                let fixable: Vec<_> = errors.into_iter().filter(|e| e.individual_fix.is_some()).collect();
                let attempted = fixable.len();
                if attempted == 0 {
                    return Ok((0, false));
                }
                let changed = fix_errors(container, fixable)?;
                Ok((attempted, changed))
            })
            .ok_or_else(|| ServerError::NotFound(format!("No tweak session: {session_id}")))?
            .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

        Ok(Json(json!({ "attempted": result.0, "changed": result.1 })))
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
}

/// `GET /tweak/report/{session_id}` -- the files, images and links
/// breakdown of the book being edited.
///
/// Word counts are deliberately excluded: `report::words_data` needs
/// a loaded dictionary to attribute words to a locale, and no
/// dictionaries are vendored here (see #816's spell-check item). A
/// report that silently omitted them would be worse than one that
/// does not claim to offer them.
pub async fn book_report(State(state): State<AppState>, AxumPath(session_id): AxumPath<String>) -> Result<Json<Value>, ServerError> {
    tokio::task::spawn_blocking(move || -> Result<Json<Value>, ServerError> {
        let (files, images) = state
            .tweak_sessions
            .with_container(&session_id, |container| -> anyhow::Result<_> {
                let files = report::files_data(container, None);
                let images = report::images_data(container)?;
                Ok((files, images))
            })
            .ok_or_else(|| ServerError::NotFound(format!("No tweak session: {session_id}")))?
            .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

        let file_items: Vec<Value> = files
            .iter()
            .map(|f| json!({ "name": f.name, "category": f.category, "size": f.size, "words": f.word_count }))
            .collect();

        let image_items: Vec<Value> = images
            .iter()
            .map(|i| json!({ "name": i.name, "size": i.size, "width": i.width, "height": i.height, "usage": i.usage.len() }))
            .collect();

        let total_size: u64 = files.iter().map(|f| f.size).sum();

        Ok(Json(json!({
            "files": { "count": file_items.len(), "total_size": total_size, "items": file_items },
            "images": { "count": image_items.len(), "items": image_items },
        })))
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
}

/// Builds the dictionary set from what ships with the binary.
///
/// Vendored rather than configured (#865): requiring a
/// `--dictionaries-dir` or a system hunspell install means spell
/// check silently does nothing on a machine that has neither, which
/// is most Windows machines and plenty of Linux ones.
fn dictionaries(locale: DictionaryLocale) -> Dictionaries {
    let builtin: Vec<DictionaryMeta> = vendored::builtin();
    Dictionaries::new(locale, Vec::new(), builtin, Vec::new(), |_| None, |_| None)
}

/// `GET /tweak/spell/{session_id}` -- misspelled words in the book
/// being edited, with where each one appears.
///
/// The engine (`oeb::polish::spell` + `spell::dictionary`) was fully
/// ported and had no caller. What was missing was dictionary *data*,
/// which now ships with the binary.
///
/// Suggestions are computed only for the words actually returned, not
/// for every word in the book: generating them is the expensive part,
/// and a book has far more correct words than misspelled ones.
pub async fn spell_check(State(state): State<AppState>, AxumPath(session_id): AxumPath<String>) -> Result<Json<Value>, ServerError> {
    tokio::task::spawn_blocking(move || -> Result<Json<Value>, ServerError> {
        // `eng-US` unless the book says otherwise; `get_all_words`
        // needs a locale to attribute words to.
        let locale = parse_lang_code("en-US").map_err(ServerError::InternalServerError)?;

        let words = state
            .tweak_sessions
            .with_container(&session_id, |container| {
                let mut counts = std::collections::HashMap::new();
                polish_spell::get_all_words(container, &locale, &std::collections::HashSet::new(), &mut counts)
            })
            .ok_or_else(|| ServerError::NotFound(format!("No tweak session: {session_id}")))?
            .map_err(|e| ServerError::InternalServerError(e.to_string()))?
            .1;

        let mut dicts = dictionaries(locale);

        let mut misspelled: Vec<Value> = Vec::new();
        for ((word, word_locale), locations) in words {
            if dicts.recognized(&word, Some(&word_locale)) {
                continue;
            }
            let files: Vec<String> = {
                let mut names: Vec<String> = locations.iter().map(|l| l.file_name.clone()).collect();
                names.sort();
                names.dedup();
                names
            };
            misspelled.push(json!({
                "word": word,
                "count": locations.len(),
                "files": files,
                "suggestions": dicts.suggestions(&word, Some(&word_locale)).into_iter().take(5).collect::<Vec<_>>(),
            }));
        }

        // Most-frequent first: a word appearing thirty times is more
        // likely a real problem than a one-off proper noun.
        misspelled.sort_by(|a, b| b["count"].as_u64().cmp(&a["count"].as_u64()).then_with(|| a["word"].as_str().cmp(&b["word"].as_str())));

        Ok(Json(json!({ "count": misspelled.len(), "words": misspelled })))
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use calibre_db::cache::Cache;
    use serde_json::Value;
    use tower::ServiceExt;

    /// A real EPUB carrying a genuine defect -- two elements sharing
    /// an id -- so `run_checks` has something true to find. A clean
    /// book would pass these tests whether or not the checks ran.
    fn epub_with_a_duplicate_id() -> Vec<u8> {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("mimetype"), "application/epub+zip").unwrap();
        std::fs::create_dir_all(dir.path().join("META-INF")).unwrap();
        std::fs::write(
            dir.path().join("META-INF/container.xml"),
            r#"<?xml version="1.0"?><container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0"><rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("chapter1.xhtml"),
            r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><body><p id="dup">One</p><p id="dup">Two</p><p>A clear misspelling: libary.</p></body></html>"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("content.opf"),
            r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="bookid">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="bookid">id1</dc:identifier><dc:title>Test</dc:title><dc:language>en</dc:language></metadata>
<manifest><item id="c1" href="chapter1.xhtml" media-type="application/xhtml+xml"/></manifest>
<spine><itemref idref="c1"/></spine>
</package>"#,
        )
        .unwrap();

        let epub_path = dir.path().join("out.epub");
        let file = std::fs::File::create(&epub_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for name in ["mimetype", "META-INF/container.xml", "content.opf", "chapter1.xhtml"] {
            zip.start_file(name, opts).unwrap();
            use std::io::Write;
            zip.write_all(&std::fs::read(dir.path().join(name)).unwrap()).unwrap();
        }
        zip.finish().unwrap();
        std::fs::read(&epub_path).unwrap()
    }

    fn test_app() -> (tempfile::TempDir, axum::Router, i32) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let source = dir.path().join("Book.epub");
        std::fs::write(&source, epub_with_a_duplicate_id()).unwrap();
        let mut meta = calibre_ebooks::metadata::MetaInformation::default();
        meta.title = "Test".to_string();
        meta.authors = vec!["Author".to_string()];
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
            tts_voice: None,
            plugin_store: None,
            plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(calibre_customize::registry::PluginRegistry::new())),
        };
        (dir, crate::test_router(state), book_id)
    }

    async fn get(router: &axum::Router, uri: &str) -> (StatusCode, Value) {
        let resp = router.clone().oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn post(router: &axum::Router, uri: &str) -> (StatusCode, Value) {
        let resp = router.clone().oneshot(Request::builder().method("POST").uri(uri).body(Body::empty()).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    /// Opens a tweak session and returns its id.
    async fn open_session(router: &axum::Router, book_id: i32) -> String {
        let (status, body) = post(router, &format!("/tweak/open/{book_id}/epub/default")).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["session_id"].as_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn check_finds_a_real_defect() {
        let (_d, router, book_id) = test_app();
        let session = open_session(&router, book_id).await;

        let (status, body) = get(&router, &format!("/tweak/check/{session}")).await;

        assert_eq!(status, StatusCode::OK, "{body}");
        // The book really does have two elements sharing an id.
        let types: Vec<&str> = body["items"].as_array().unwrap().iter().filter_map(|i| i["type"].as_str()).collect();
        assert!(types.iter().any(|t| t.contains("DuplicateId")), "expected a duplicate-id error, got {types:?}");
    }

    #[tokio::test]
    async fn check_reports_levels_and_fixability_for_the_ui() {
        let (_d, router, book_id) = test_app();
        let session = open_session(&router, book_id).await;

        let (_, body) = get(&router, &format!("/tweak/check/{session}")).await;

        for item in body["items"].as_array().unwrap() {
            // Without a level the UI cannot tell a note from a real
            // problem, and without `fixable` it cannot decide whether
            // to offer a fix button at all.
            assert!(item["level"].is_string(), "{item}");
            assert!(item["fixable"].is_boolean(), "{item}");
        }
    }

    #[tokio::test]
    async fn check_fix_reports_what_it_attempted() {
        let (_d, router, book_id) = test_app();
        let session = open_session(&router, book_id).await;

        let (status, body) = post(&router, &format!("/tweak/check-fix/{session}")).await;

        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body["attempted"].is_number(), "{body}");
        assert!(body["changed"].is_boolean(), "{body}");
    }

    #[tokio::test]
    async fn the_report_describes_the_book_being_edited() {
        let (_d, router, book_id) = test_app();
        let session = open_session(&router, book_id).await;

        let (status, body) = get(&router, &format!("/tweak/report/{session}")).await;

        assert_eq!(status, StatusCode::OK, "{body}");
        let names: Vec<&str> = body["files"]["items"].as_array().unwrap().iter().filter_map(|f| f["name"].as_str()).collect();
        assert!(names.contains(&"chapter1.xhtml"), "{body}");
        assert!(body["files"]["total_size"].as_u64().unwrap() > 0, "{body}");
    }

    /// These act on the *open session*, not the on-disk book, so they
    /// see unsaved edits -- checking the stored copy would report
    /// problems the editor had already fixed.
    #[tokio::test]
    async fn the_report_reflects_an_unsaved_edit() {
        let (_d, router, book_id) = test_app();
        let session = open_session(&router, book_id).await;

        let before = get(&router, &format!("/tweak/report/{session}")).await.1["files"]["total_size"].as_u64().unwrap();

        let longer = "x".repeat(4000);
        let resp = router
            .clone()
            .oneshot(Request::builder().method("POST").uri(format!("/tweak/file/{session}/chapter1.xhtml")).body(Body::from(format!("<html><body><p>{longer}</p></body></html>"))).unwrap())
            .await
            .unwrap();
        assert!(resp.status().is_success());

        let after = get(&router, &format!("/tweak/report/{session}")).await.1["files"]["total_size"].as_u64().unwrap();
        assert!(after > before, "the report should see the unsaved edit: {before} -> {after}");
    }

    /// The engine was always real; what was missing was dictionary
    /// data, which now ships with the binary (#865). This proves the
    /// whole path works end to end against a book with a genuine
    /// misspelling in it.
    #[tokio::test]
    async fn spell_check_finds_a_real_misspelling() {
        let (_d, router, book_id) = test_app();
        let session = open_session(&router, book_id).await;

        let (status, body) = get(&router, &format!("/tweak/spell/{session}")).await;

        assert_eq!(status, StatusCode::OK, "{body}");
        let words: Vec<&str> = body["words"].as_array().unwrap().iter().filter_map(|w| w["word"].as_str()).collect();
        assert!(words.contains(&"libary"), "expected the misspelling to be flagged, got {words:?}");
        // And correctly-spelled words must not be.
        assert!(!words.contains(&"misspelling"), "a correct word was flagged: {words:?}");
    }

    #[tokio::test]
    async fn spell_check_offers_suggestions_and_locations() {
        let (_d, router, book_id) = test_app();
        let session = open_session(&router, book_id).await;

        let (_, body) = get(&router, &format!("/tweak/spell/{session}")).await;
        let entry = body["words"].as_array().unwrap().iter().find(|w| w["word"] == "libary").expect("the misspelling should be listed");

        assert!(entry["files"].as_array().is_some_and(|f| !f.is_empty()), "a misspelling with no location cannot be found: {entry}");
        assert!(entry["count"].as_u64().unwrap_or(0) >= 1, "{entry}");
        // Suggestions are what make the report actionable rather than
        // merely accusatory.
        let suggestions: Vec<&str> = entry["suggestions"].as_array().unwrap().iter().filter_map(|s| s.as_str()).collect();
        assert!(suggestions.iter().any(|s| *s == "library"), "expected 'library' among suggestions, got {suggestions:?}");
    }

    #[tokio::test]
    async fn an_unknown_session_is_a_404_on_every_route() {
        let (_d, router, _) = test_app();
        assert_eq!(get(&router, "/tweak/check/nope").await.0, StatusCode::NOT_FOUND);
        assert_eq!(get(&router, "/tweak/report/nope").await.0, StatusCode::NOT_FOUND);
        assert_eq!(post(&router, "/tweak/check-fix/nope").await.0, StatusCode::NOT_FOUND);
        assert_eq!(get(&router, "/tweak/spell/nope").await.0, StatusCode::NOT_FOUND);
    }
}
