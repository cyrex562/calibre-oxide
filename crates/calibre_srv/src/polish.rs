//! `POST /polish/{library_id}` -- a real, **new** route (issue 1.10
//! of the #816 epic).
//!
//! # The engine had no caller at all
//!
//! `oeb::polish` is 92 files of ported, tested engine, and
//! `polish_one` -- its entry point -- was called by nothing: no
//! route, no CLI, no UI. Confirmed by grep before writing this.
//! Upstream drives polishing from a Qt dialog and from
//! `ebook-polish`, so there is no `calibre.srv` endpoint to port.
//!
//! # Scope
//!
//! EPUB only, matching what this crate can actually open:
//! `EpubContainer::open_zip` is the one container this server already
//! knows how to open and commit (see `tweak.rs`). `polish_one` itself
//! also handles KEPUB and AZW3 containers, so widening is a matter of
//! opening them, not of engine work.
//!
//! `PolishOptions::opf` is not exposed: `polish_one` bails on it with
//! its own disclosed narrowing ("no in-place OPF `<metadata>`
//! rewriter"), so offering it in the UI would only produce an error.
//!
//! # Synchronous, deliberately
//!
//! This runs the whole request inline rather than through a job
//! registry like `convert.rs`. Polishing a handful of selected books
//! takes seconds, which is the shape a desktop user actually
//! triggers; a job registry is the right follow-up if someone starts
//! polishing thousands at once, not speculative scaffolding now.
//! `MAX_BOOKS` keeps a single request bounded either way.

use axum::extract::{Path, State};
use axum::Json;
use calibre_ebooks::oeb::polish::container::{AnyContainer, EpubContainer};
use calibre_ebooks::oeb::polish::main::{polish_one, PolishCustomization, PolishOptions};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::errors::ServerError;
use crate::AppState;

/// One request cannot polish more than this many books. Each one is
/// opened, rewritten and committed, so an unbounded request would
/// hold a connection open for an arbitrarily long time.
const MAX_BOOKS: usize = 200;

/// The subset of `PolishOptions` worth exposing. Named to match the
/// engine's own fields so the mapping stays obvious.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PolishRequestOptions {
    pub jacket: bool,
    pub remove_jacket: bool,
    pub smarten_punctuation: bool,
    pub remove_unused_css: bool,
    pub compress_images: bool,
    pub upgrade_book: bool,
    pub add_soft_hyphens: bool,
    pub remove_soft_hyphens: bool,
    pub download_external_resources: bool,
    pub embed: bool,
    pub subset: bool,
    // `PolishCustomization`, which only matters alongside
    // `remove_unused_css`.
    pub remove_unused_classes: bool,
    pub merge_identical_selectors: bool,
    pub merge_rules_with_identical_properties: bool,
    pub remove_unreferenced_sheets: bool,
    pub remove_ncx: bool,
}

impl PolishRequestOptions {
    /// Whether any action was actually requested. An all-false
    /// request would open and rewrite every book to do nothing.
    fn requests_anything(&self) -> bool {
        self.jacket
            || self.remove_jacket
            || self.smarten_punctuation
            || self.remove_unused_css
            || self.compress_images
            || self.upgrade_book
            || self.add_soft_hyphens
            || self.remove_soft_hyphens
            || self.download_external_resources
            || self.embed
            || self.subset
    }

    fn to_engine(&self) -> (PolishOptions, PolishCustomization) {
        let opts = PolishOptions {
            cover: None,
            opf: None,
            jacket: self.jacket,
            remove_jacket: self.remove_jacket,
            smarten_punctuation: self.smarten_punctuation,
            remove_unused_css: self.remove_unused_css,
            compress_images: self.compress_images,
            upgrade_book: self.upgrade_book,
            add_soft_hyphens: self.add_soft_hyphens,
            remove_soft_hyphens: self.remove_soft_hyphens,
            download_external_resources: self.download_external_resources,
            embed: self.embed,
            subset: self.subset,
        };
        let customization = PolishCustomization {
            remove_unused_classes: self.remove_unused_classes,
            merge_identical_selectors: self.merge_identical_selectors,
            merge_rules_with_identical_properties: self.merge_rules_with_identical_properties,
            remove_unreferenced_sheets: self.remove_unreferenced_sheets,
            remove_ncx: self.remove_ncx,
        };
        (opts, customization)
    }
}

#[derive(Debug, Deserialize)]
pub struct PolishBody {
    pub book_ids: Vec<i32>,
    #[serde(default)]
    pub options: PolishRequestOptions,
}

async fn handle(state: AppState, library_id: Option<&str>, body: PolishBody) -> Result<Json<Value>, ServerError> {
    if body.book_ids.is_empty() {
        return Err(ServerError::BadRequest("book_ids must not be empty".to_string()));
    }
    if body.book_ids.len() > MAX_BOOKS {
        return Err(ServerError::BadRequest(format!("at most {MAX_BOOKS} books can be polished in one request (got {})", body.book_ids.len())));
    }
    // Every option off would open, rewrite and re-add every book in
    // order to change nothing -- almost certainly a UI bug, and
    // expensive enough to be worth refusing loudly.
    if !body.options.requests_anything() {
        return Err(ServerError::BadRequest("no polish actions were requested".to_string()));
    }

    let cache = state.cache_for(library_id).ok_or_else(|| ServerError::NotFound(format!("no library named {:?}", library_id.unwrap_or(""))))?;
    let (opts, customization) = body.options.to_engine();
    let book_ids = body.book_ids.clone();

    let results = tokio::task::spawn_blocking(move || -> Vec<Value> {
        let mut out = Vec::new();
        for book_id in book_ids {
            out.push(polish_book(&cache, book_id, &opts, &customization));
        }
        out
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    let changed = results.iter().filter(|r| r["changed"] == json!(true)).count();
    let failed = results.iter().filter(|r| r.get("error").is_some()).count();
    Ok(Json(json!({ "changed": changed, "failed": failed, "books": results })))
}

/// Polishes one book, reporting per-book rather than failing the
/// whole request: one unopenable book out of fifty should not discard
/// the other forty-nine's work.
fn polish_book(cache: &calibre_db::cache::Cache, book_id: i32, opts: &PolishOptions, customization: &PolishCustomization) -> Value {
    match polish_book_inner(cache, book_id, opts, customization) {
        Ok((changed, report)) => json!({ "book_id": book_id, "changed": changed, "report": report }),
        Err(e) => json!({ "book_id": book_id, "changed": false, "error": e.to_string() }),
    }
}

fn polish_book_inner(cache: &calibre_db::cache::Cache, book_id: i32, opts: &PolishOptions, customization: &PolishCustomization) -> anyhow::Result<(bool, Vec<String>)> {
    let ids: std::collections::HashSet<i32> = std::iter::once(book_id).collect();
    let rows = cache.get_data_as_dict(None, true, Some(&ids), false)?;
    let row = rows.into_iter().next().ok_or_else(|| anyhow::anyhow!("no book with id {book_id}"))?;
    let path_str = row.get("fmt_epub").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("no epub format for book {book_id} (this port can only polish epub)"))?;
    let path = std::path::PathBuf::from(path_str);

    let tdir = tempfile::tempdir()?;
    let mut container = AnyContainer::Epub(EpubContainer::open_zip(&path, tdir.path())?);

    let mut report: Vec<String> = Vec::new();
    let changed = {
        let mut collect = |line: &str| {
            let line = line.trim();
            if !line.is_empty() {
                report.push(line.to_string());
            }
        };
        polish_one(&mut container, opts, &mut collect, Some(customization))?
    };

    // Only write back when something really changed -- re-adding an
    // identical file would bump the book's timestamp for nothing.
    if changed {
        let out_dir = tempfile::tempdir()?;
        let out_path = out_dir.path().join("polished.epub");
        match &mut container {
            AnyContainer::Epub(c) => c.commit(Some(&out_path))?,
            _ => anyhow::bail!("only epub containers can be committed by this route"),
        }
        cache.add_format(book_id, &out_path, "epub", true)?;
    }

    Ok((changed, report))
}

pub async fn polish(State(state): State<AppState>, Json(body): Json<PolishBody>) -> Result<Json<Value>, ServerError> {
    handle(state, None, body).await
}

pub async fn polish_for_library(State(state): State<AppState>, Path(library_id): Path<String>, Json(body): Json<PolishBody>) -> Result<Json<Value>, ServerError> {
    handle(state, Some(&library_id), body).await
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use calibre_db::cache::Cache;
    use serde_json::json;
    use tower::ServiceExt;

    use super::*;

    /// A minimal but real, valid EPUB -- built the same way the tweak
    /// tests do. Polishing a stub would prove nothing, since every
    /// action here parses real OPF/XHTML.
    fn real_test_epub_bytes() -> Vec<u8> {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("mimetype"), "application/epub+zip").unwrap();
        std::fs::create_dir_all(dir.path().join("META-INF")).unwrap();
        std::fs::write(
            dir.path().join("META-INF/container.xml"),
            r#"<?xml version="1.0"?><container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0"><rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
        )
        .unwrap();
        // Straight quotes and a double hyphen so smarten_punctuation
        // has something real to change.
        std::fs::write(
            dir.path().join("chapter1.xhtml"),
            r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><body><p>He said "hello" -- and left...</p></body></html>"#,
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

    fn test_app(with_epub: bool) -> (tempfile::TempDir, axum::Router, i32) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let ext = if with_epub { "epub" } else { "txt" };
        let source = dir.path().join(format!("Book.{ext}"));
        if with_epub {
            std::fs::write(&source, real_test_epub_bytes()).unwrap();
        } else {
            std::fs::write(&source, b"plain text").unwrap();
        }
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

    async fn post(router: &axum::Router, uri: &str, body: Value) -> (StatusCode, Value) {
        let req = Request::builder().method("POST").uri(uri).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    /// The headline: the 92-file engine actually runs and rewrites a
    /// real book through this route.
    #[tokio::test]
    async fn smarten_punctuation_really_changes_the_book() {
        let (_d, router, book_id) = test_app(true);

        let (status, body) = post(&router, "/polish", json!({"book_ids": [book_id], "options": {"smarten_punctuation": true}})).await;

        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["changed"], 1, "{body}");
        assert_eq!(body["failed"], 0, "{body}");
        assert!(body["books"][0]["report"].as_array().is_some_and(|r| !r.is_empty()), "the engine should report what it did: {body}");

        // `changed: true` is only the engine's own say-so. Read the
        // stored EPUB back and confirm the straight quotes and double
        // hyphen really became typographic ones on disk.
        let resp = router.clone().oneshot(Request::builder().uri(format!("/get/epub/{book_id}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let epub = to_bytes(resp.into_body(), usize::MAX).await.unwrap();

        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(epub.to_vec())).unwrap();
        let mut chapter = String::new();
        {
            use std::io::Read;
            zip.by_name("chapter1.xhtml").unwrap().read_to_string(&mut chapter).unwrap();
        }
        assert!(chapter.contains('\u{201c}') || chapter.contains('\u{201d}'), "quotes should have been smartened on disk: {chapter}");
        assert!(!chapter.contains("--"), "the double hyphen should have become a dash: {chapter}");
    }

    #[tokio::test]
    async fn an_all_false_request_is_refused_rather_than_rewriting_every_book_for_nothing() {
        let (_d, router, book_id) = test_app(true);

        let (status, _) = post(&router, "/polish", json!({"book_ids": [book_id], "options": {}})).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn an_empty_book_list_is_refused() {
        let (_d, router, _) = test_app(true);
        assert_eq!(post(&router, "/polish", json!({"book_ids": [], "options": {"smarten_punctuation": true}})).await.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn too_many_books_at_once_is_refused() {
        let (_d, router, _) = test_app(true);
        let ids: Vec<i32> = (1..=(MAX_BOOKS as i32 + 1)).collect();
        assert_eq!(post(&router, "/polish", json!({"book_ids": ids, "options": {"smarten_punctuation": true}})).await.0, StatusCode::BAD_REQUEST);
    }

    /// One unpolishable book must not discard the rest of the batch's
    /// work, so failures are reported per book rather than aborting.
    #[tokio::test]
    async fn a_book_without_an_epub_is_reported_not_fatal() {
        let (_d, router, book_id) = test_app(false);

        let (status, body) = post(&router, "/polish", json!({"book_ids": [book_id], "options": {"smarten_punctuation": true}})).await;

        assert_eq!(status, StatusCode::OK, "the request itself should succeed: {body}");
        assert_eq!(body["failed"], 1, "{body}");
        assert!(body["books"][0]["error"].as_str().is_some_and(|e| e.contains("epub")), "{body}");
    }

    #[tokio::test]
    async fn an_unknown_book_is_reported_not_fatal() {
        let (_d, router, _) = test_app(true);

        let (status, body) = post(&router, "/polish", json!({"book_ids": [9999], "options": {"smarten_punctuation": true}})).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["failed"], 1, "{body}");
    }

    #[test]
    fn requests_anything_ignores_the_customization_only_flags() {
        // `remove_unused_classes` and friends only modify how
        // `remove_unused_css` behaves. On their own they request no
        // action, and treating them as one would rewrite every book
        // to do nothing.
        let customization_only = PolishRequestOptions { remove_unused_classes: true, merge_identical_selectors: true, remove_ncx: true, ..Default::default() };
        assert!(!customization_only.requests_anything());

        let real = PolishRequestOptions { remove_unused_css: true, ..Default::default() };
        assert!(real.requests_anything());
    }

    #[test]
    fn every_exposed_option_reaches_the_engine() {
        // Guards a field added to the request struct but forgotten in
        // `to_engine`, which would silently ignore what the user
        // asked for.
        let all_on = PolishRequestOptions {
            jacket: true,
            remove_jacket: true,
            smarten_punctuation: true,
            remove_unused_css: true,
            compress_images: true,
            upgrade_book: true,
            add_soft_hyphens: true,
            remove_soft_hyphens: true,
            download_external_resources: true,
            embed: true,
            subset: true,
            remove_unused_classes: true,
            merge_identical_selectors: true,
            merge_rules_with_identical_properties: true,
            remove_unreferenced_sheets: true,
            remove_ncx: true,
        };
        let (opts, custom) = all_on.to_engine();

        assert!(opts.jacket && opts.remove_jacket && opts.smarten_punctuation && opts.remove_unused_css);
        assert!(opts.compress_images && opts.upgrade_book && opts.add_soft_hyphens && opts.remove_soft_hyphens);
        assert!(opts.download_external_resources && opts.embed && opts.subset);
        assert!(custom.remove_unused_classes && custom.merge_identical_selectors);
        assert!(custom.merge_rules_with_identical_properties && custom.remove_unreferenced_sheets && custom.remove_ncx);
        // Deliberately not exposed: `polish_one` bails on it.
        assert!(opts.opf.is_none());
    }
}
