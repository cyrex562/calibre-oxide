//! A travel copy and a backup mirror of the library ([`lexmechanic_db::mirror`]).
//!
//! - `POST /mirror-library/{library_id}` `{target, mode: "travel"|"mirror",
//!   scope?: {all} | {book_ids} | {folder_prefix}, remove_missing?,
//!   overwrite_changed?, dry_run?, fingerprint?}` -- `dry_run` defaults to
//!   **true** and answers the plan without touching the target:
//!   `{fingerprint, mode, target, new_target, tier, network, books, totals,
//!   items: {copy_new, update, remove, conflict}, truncated, copy_edits,
//!   missing_sources}`. `items` lists at most 500 entries per action; `totals`
//!   (counts and bytes, overall and per kind) are always complete.
//!   `dry_run: false` needs the `fingerprint` of the dry run the user confirmed,
//!   plans again, answers `409` if the plan is no longer that one, and
//!   otherwise starts a `mirror` job (202; progress and the report at
//!   `/library-jobs/{id}`: `added` is files written, `bytes_hashed` bytes,
//!   `total` files to write). A target that is refused (inside the library,
//!   not empty and not a copy, a copy of another library, open right now, ...)
//!   is `400` with the reason.
//! - `GET /mirror-targets/{library_id}` -- the remembered targets (library
//!   preference `lexmechanic.mirror_targets`): mode, scope, options, when they last
//!   ran and what came of it, and whether the target answers right now.
//! - `POST /mirror-targets/remove/{library_id}` `{target}` -- forget one. It
//!   never deletes a file.
//!
//! A target is remembered when a run ends (finished, cancelled or failed), not
//! when it is previewed. An interrupted run is continued by running the target
//! again: what it finished is in the copy's manifest.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use lexmechanic_db::cache::Cache;
use lexmechanic_db::mirror::{self, Action, MirrorError, MirrorOptions, MirrorPlan, MirrorScope, Mode};
use lexmechanic_db::scan_pipeline::JobControl;

use crate::errors::ServerError;
use crate::library_jobs::{self, JobKind, JobParams};
use crate::AppState;

const MAX_LISTED: usize = 500;
/// Book ids kept in a remembered scope; beyond that the scope is not stored.
const MAX_REMEMBERED_IDS: usize = 2000;

fn default_true() -> bool {
    true
}

#[derive(Debug, Default, Deserialize)]
pub struct ScopeBody {
    pub all: Option<bool>,
    pub book_ids: Option<Vec<i32>>,
    pub folder_prefix: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MirrorBody {
    pub target: String,
    pub mode: String,
    pub scope: Option<ScopeBody>,
    pub remove_missing: Option<bool>,
    pub overwrite_changed: Option<bool>,
    #[serde(default = "default_true")]
    pub dry_run: bool,
    pub fingerprint: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ForgetBody {
    pub target: String,
}

fn internal(e: impl std::fmt::Display) -> ServerError {
    ServerError::InternalServerError(e.to_string())
}

fn options_of(body: &MirrorBody) -> Result<MirrorOptions, ServerError> {
    let mode = match body.mode.as_str() {
        "travel" => Mode::Travel,
        "mirror" => Mode::Mirror,
        other => return Err(ServerError::BadRequest(format!("mode must be \"travel\" or \"mirror\", not {other:?}"))),
    };
    let scope = match &body.scope {
        None => MirrorScope::All,
        Some(s) => match (&s.book_ids, &s.folder_prefix) {
            (Some(_), Some(_)) => return Err(ServerError::BadRequest("give one of book_ids or folder_prefix (or all)".into())),
            (Some(ids), None) if ids.is_empty() => return Err(ServerError::BadRequest("book_ids is empty".into())),
            (Some(ids), None) => MirrorScope::BookIds(ids.clone()),
            (None, Some(prefix)) if prefix.trim().is_empty() => return Err(ServerError::BadRequest("folder_prefix is empty".into())),
            (None, Some(prefix)) => MirrorScope::FolderPrefix(prefix.clone()),
            (None, None) => MirrorScope::All,
        },
    };
    if mode == Mode::Mirror && scope != MirrorScope::All {
        return Err(ServerError::BadRequest("a backup mirror always holds the whole library; use a travel copy for some of the books".into()));
    }
    Ok(MirrorOptions { mode, scope, remove_missing: body.remove_missing.unwrap_or(true), overwrite_changed: body.overwrite_changed.unwrap_or(false) })
}

fn plan_error(e: MirrorError) -> ServerError {
    match e {
        MirrorError::Refused(message) => ServerError::BadRequest(message),
        MirrorError::Other(e) => internal(format!("{e:#}")),
    }
}

fn listed(plan: &MirrorPlan, action: Action) -> (Vec<Value>, bool) {
    let all: Vec<&mirror::PlanItem> = plan.items_with(action).collect();
    let items = all.iter().take(MAX_LISTED).map(|i| json!({"path": i.rel, "kind": i.kind.name(), "bytes": i.size, "reason": i.reason})).collect();
    (items, all.len() > MAX_LISTED)
}

/// What a dry run answers.
pub fn plan_json(plan: &MirrorPlan) -> Value {
    let mut items = serde_json::Map::new();
    let mut truncated = serde_json::Map::new();
    for action in [Action::CopyNew, Action::Update, Action::Remove, Action::Conflict] {
        let (list, cut) = listed(plan, action);
        items.insert(action.name().into(), json!(list));
        truncated.insert(action.name().into(), json!(cut));
    }
    json!({
        "fingerprint": plan.fingerprint,
        "mode": plan.mode.name(),
        "target": plan.target.display().to_string(),
        "new_target": plan.new_target,
        "tier": lexmechanic_db::storage_safety::tier_name(plan.tier),
        "network": plan.tier == lexmechanic_db::library_handle::StorageTier::Network,
        "books": plan.books,
        "scope": plan.options.scope.label(),
        "remove_missing": plan.options.remove_missing,
        "overwrite_changed": plan.options.overwrite_changed,
        "totals": plan.totals,
        "items": items,
        "truncated": truncated,
        "copy_edits": {"count": plan.copy_edits.len(), "paths": plan.copy_edits.iter().take(50).collect::<Vec<_>>()},
        "missing_sources": {"count": plan.missing_sources.len(), "paths": plan.missing_sources.iter().take(50).collect::<Vec<_>>()},
    })
}

fn started(job: library_jobs::Started) -> Response {
    match job {
        library_jobs::Started::New(job) | library_jobs::Started::AlreadyRunning(job) => (StatusCode::ACCEPTED, Json(json!({"job_id": job.id, "kind": "mirror", "state": "running", "status_url": format!("/library-jobs/{}", job.id)}))).into_response(),
        library_jobs::Started::Busy(active) => (StatusCode::CONFLICT, Json(json!({"error": "another job is already running on this library", "job_id": active.id, "kind": active.kind.name()}))).into_response(),
    }
}

/// `POST /mirror-library/{library_id}`.
pub async fn mirror_library(State(state): State<AppState>, Path(library_id): Path<String>, Json(body): Json<MirrorBody>) -> Result<Response, ServerError> {
    let cache = state.cache_for(Some(&library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))?;
    let options = options_of(&body)?;
    if !body.dry_run && body.fingerprint.is_none() {
        return Err(ServerError::BadRequest("executing needs the fingerprint of the dry run the user confirmed".into()));
    }
    let (plan_cache, target) = (cache.clone(), body.target.clone());
    let plan = tokio::task::spawn_blocking(move || mirror::plan(&plan_cache, &target, &options)).await.map_err(internal)?.map_err(plan_error)?;

    if body.dry_run {
        return Ok(Json(plan_json(&plan)).into_response());
    }
    if body.fingerprint.as_deref() != Some(plan.fingerprint.as_str()) {
        return Ok((StatusCode::CONFLICT, Json(json!({"error": "the copy would no longer be what was previewed (the library or the copy changed); preview it again", "fingerprint": plan.fingerprint}))).into_response());
    }
    let params = JobParams { mirror: Some(Arc::new(plan)), ..Default::default() };
    Ok(started(library_jobs::start_with(cache, JobKind::Mirror, None, params)))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn load_targets(cache: &Cache) -> Vec<Value> {
    cache.pref(mirror::TARGETS_PREF, None).or_else(|| cache.pref(mirror::LEGACY_TARGETS_PREF, None)).and_then(|v| v.as_array().cloned()).unwrap_or_default()
}

/// Written to the database only, not to the change log: where copies go is
/// machine-local, and logging it would make every run add an entry to the log
/// a mirror then has to copy (and a rebuild would bring back another
/// machine's folders).
fn store_targets(cache: &Cache, targets: &[Value]) {
    if let Err(e) = cache.backend.set_pref(mirror::TARGETS_PREF, &Value::Array(targets.to_vec())) {
        eprintln!("could not remember the copy targets: {e:#}");
        return;
    }
    // The list now lives under the new name only.
    let _ = cache.backend.delete_pref(mirror::LEGACY_TARGETS_PREF);
}

fn same_target(entry: &Value, target: &str) -> bool {
    let norm = |s: &str| s.trim().trim_end_matches(['/', '\\']).replace('\\', "/");
    entry["target"].as_str().is_some_and(|t| if cfg!(windows) { norm(t).eq_ignore_ascii_case(&norm(target)) } else { norm(t) == norm(target) })
}

/// Records how a run went in the remembered targets (replacing the entry for the
/// same folder).
fn remember(cache: &Cache, plan: &MirrorPlan, outcome: &Result<mirror::MirrorReport, String>) {
    let target = plan.target.display().to_string();
    let scope = match &plan.options.scope {
        MirrorScope::BookIds(ids) if ids.len() > MAX_REMEMBERED_IDS => json!({"too_large": ids.len()}),
        other => serde_json::to_value(other).unwrap_or(Value::Null),
    };
    let result = match outcome {
        Ok(r) => json!({"copied": r.copied, "updated": r.updated, "unchanged": r.unchanged, "removed": r.removed, "conflicts": r.conflicts.len(), "failures": r.failures.len(), "bytes_copied": r.bytes_copied, "complete": r.complete, "cancelled": r.cancelled}),
        Err(message) => json!({"complete": false, "error": message}),
    };
    let entry = json!({
        "target": target,
        "mode": plan.mode.name(),
        "scope": scope,
        "remove_missing": plan.options.remove_missing,
        "overwrite_changed": plan.options.overwrite_changed,
        "last_run_at": now_secs(),
        "last_run_id": outcome.as_ref().ok().map(|r| r.run_id.clone()),
        "last_result": result,
    });
    let mut targets = load_targets(cache);
    targets.retain(|t| !same_target(t, &target));
    targets.insert(0, entry);
    store_targets(cache, &targets);
}

/// The body of a [`JobKind::Mirror`] job: runs the plan and remembers the target.
pub fn run_job(cache: &Cache, plan: &MirrorPlan, control: &JobControl) -> anyhow::Result<Value> {
    let outcome = mirror::execute(cache, plan, control);
    remember(cache, plan, &outcome.as_ref().map(Clone::clone).map_err(|e| format!("{e:#}")));
    Ok(serde_json::to_value(outcome?)?)
}

/// `GET /mirror-targets/{library_id}`.
pub async fn targets(State(state): State<AppState>, Path(library_id): Path<String>) -> Result<Json<Value>, ServerError> {
    let cache = state.cache_for(Some(&library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))?;
    let mut entries = tokio::task::spawn_blocking(move || load_targets(&cache)).await.map_err(internal)?;
    // A share that is down can take a minute to say so: ask for all of them at
    // once, and give each a few seconds.
    let probes: Vec<_> = entries
        .iter()
        .map(|e| {
            let path = e["target"].as_str().unwrap_or("").to_string();
            tokio::task::spawn_blocking(move || !path.is_empty() && std::path::Path::new(&path).is_dir())
        })
        .collect();
    for (entry, probe) in entries.iter_mut().zip(probes) {
        let reachable = matches!(tokio::time::timeout(Duration::from_secs(3), probe).await, Ok(Ok(true)));
        entry["reachable"] = json!(reachable);
    }
    Ok(Json(json!({"targets": entries})))
}

/// `POST /mirror-targets/remove/{library_id}`.
pub async fn forget_target(State(state): State<AppState>, Path(library_id): Path<String>, Json(body): Json<ForgetBody>) -> Result<Json<Value>, ServerError> {
    let cache = state.cache_for(Some(&library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))?;
    let left = tokio::task::spawn_blocking(move || {
        let mut targets = load_targets(&cache);
        targets.retain(|t| !same_target(t, &body.target));
        store_targets(&cache, &targets);
        targets
    })
    .await
    .map_err(internal)?;
    Ok(Json(json!({"targets": left})))
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use lexmechanic_db::cache::Cache;
    use serde_json::{json, Value};
    use tower::ServiceExt;

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
            plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(lexmechanic_customize::registry::PluginRegistry::new())),
        };
        (cache, crate::test_router(state))
    }

    fn add(cache: &Cache, scratch: &std::path::Path, title: &str, body: &str) -> i32 {
        let file = scratch.join(format!("{title}.txt"));
        std::fs::write(&file, body).unwrap();
        let mut meta = lexmechanic_ebooks::metadata::MetaInformation::default();
        meta.title = title.to_string();
        meta.authors = vec!["Some Author".to_string()];
        cache.add_book(&file, &meta).unwrap()
    }

    async fn call(router: &axum::Router, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
        let req = Request::builder().method(method).uri(uri).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn call_text(router: &axum::Router, uri: &str, body: Value) -> (StatusCode, String) {
        let req = Request::builder().method("POST").uri(uri).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8_lossy(&bytes).to_string())
    }

    async fn get(router: &axum::Router, uri: &str) -> Value {
        let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    }

    async fn finished(router: &axum::Router, started: &Value) -> Value {
        let id = started["job_id"].as_u64().unwrap_or_else(|| panic!("no job id in {started}"));
        for _ in 0..400 {
            let job = get(router, &format!("/library-jobs/{id}")).await;
            if job["state"] != "running" {
                return job;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("job {id} never finished");
    }

    fn sample() -> (tempfile::TempDir, std::sync::Arc<Cache>, axum::Router, tempfile::TempDir, Vec<i32>) {
        let dir = tempfile::tempdir().unwrap();
        let (cache, router) = app(dir.path());
        let scratch = tempfile::tempdir().unwrap();
        let ids = vec![add(&cache, scratch.path(), "Alpha", "alpha"), add(&cache, scratch.path(), "Beta", "beta"), add(&cache, scratch.path(), "Gamma", "gamma")];
        (dir, cache, router, scratch, ids)
    }

    #[tokio::test]
    async fn a_dry_run_answers_the_plan_and_writes_nothing() {
        let (_dir, _cache, router, _s, _ids) = sample();
        let out = tempfile::tempdir().unwrap();
        let target = out.path().join("backup");
        let (status, plan) = call(&router, "POST", "/mirror-library/default", json!({"target": target.to_str().unwrap(), "mode": "mirror"})).await;
        assert_eq!(status, StatusCode::OK, "{plan}");
        assert_eq!(plan["new_target"], true);
        assert_eq!(plan["books"], 3);
        assert!(plan["totals"]["copy_new"]["files"].as_u64().unwrap() >= 4, "{plan}");
        assert_eq!(plan["totals"]["conflict"]["files"], 0);
        assert!(plan["items"]["copy_new"].as_array().unwrap().iter().any(|i| i["kind"] == "format"));
        assert!(plan["fingerprint"].is_string());
        assert!(!target.exists());
    }

    #[tokio::test]
    async fn executing_needs_the_previewed_fingerprint_and_409s_when_it_changed() {
        let (dir, cache, router, scratch, _ids) = sample();
        let out = tempfile::tempdir().unwrap();
        let body = |extra: Value| {
            let mut b = json!({"target": out.path().join("b").to_str().unwrap(), "mode": "mirror"});
            for (k, v) in extra.as_object().unwrap() {
                b[k] = v.clone();
            }
            b
        };
        let (_, dry) = call(&router, "POST", "/mirror-library/default", body(json!({}))).await;
        let (status, _) = call(&router, "POST", "/mirror-library/default", body(json!({"dry_run": false}))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "executing needs a fingerprint");
        let (status, again) = call(&router, "POST", "/mirror-library/default", body(json!({"dry_run": false, "fingerprint": "nope"}))).await;
        assert_eq!(status, StatusCode::CONFLICT, "{again}");
        assert_eq!(again["fingerprint"], dry["fingerprint"], "the 409 hands back the current plan's fingerprint");

        // The library moves on after the preview.
        add(&cache, scratch.path(), "Delta", "delta");
        let (status, _) = call(&router, "POST", "/mirror-library/default", body(json!({"dry_run": false, "fingerprint": dry["fingerprint"]}))).await;
        assert_eq!(status, StatusCode::CONFLICT, "a changed library is a different plan");
        drop(dir);
    }

    #[tokio::test]
    async fn a_run_is_a_job_that_copies_remembers_the_target_and_is_cheap_the_second_time() {
        let (_dir, _cache, router, _s, _ids) = sample();
        let out = tempfile::tempdir().unwrap();
        let target = out.path().join("backup");
        let req = json!({"target": target.to_str().unwrap(), "mode": "mirror"});
        let (_, dry) = call(&router, "POST", "/mirror-library/default", req.clone()).await;
        let (status, started) = call(&router, "POST", "/mirror-library/default", json!({"target": target.to_str().unwrap(), "mode": "mirror", "dry_run": false, "fingerprint": dry["fingerprint"]})).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{started}");
        let job = finished(&router, &started).await;
        assert_eq!(job["state"], "finished", "{job}");
        assert_eq!(job["kind"], "mirror");
        assert_eq!(job["report"]["complete"], true, "{job}");
        assert_eq!(job["report"]["failures"].as_array().unwrap().len(), 0, "{job}");
        assert_eq!(job["progress"]["added"], dry["totals"]["copy_new"]["files"], "{job}");
        assert!(target.join(".lexmechanic-mirror.json").is_file());

        let (_, targets) = (StatusCode::OK, get(&router, "/mirror-targets/default").await);
        let remembered = &targets["targets"][0];
        assert_eq!(remembered["mode"], "mirror");
        assert_eq!(remembered["reachable"], true);
        assert_eq!(remembered["last_result"]["complete"], true);
        assert!(remembered["last_run_at"].as_u64().unwrap() > 0);

        // Second time: nothing to write.
        let (_, second) = call(&router, "POST", "/mirror-library/default", req).await;
        assert_eq!(second["new_target"], false);
        assert_eq!(second["totals"]["copy_new"]["files"], 0, "{second}");
        assert_eq!(second["totals"]["update"]["files"], 0, "{second}");
        assert!(second["totals"]["unchanged"]["files"].as_u64().unwrap() > 0);

        // Forgetting never touches the files.
        let (status, left) = call(&router, "POST", "/mirror-targets/remove/default", json!({"target": target.to_str().unwrap()})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(left["targets"].as_array().unwrap().len(), 0);
        assert!(target.join(".lexmechanic-mirror.json").is_file());
    }

    #[tokio::test]
    async fn an_unreachable_remembered_target_says_so() {
        let (_dir, cache, router, _s, _ids) = sample();
        cache.set_pref("lexmechanic.mirror_targets", &json!([{"target": "Z:/definitely/not/here/12345", "mode": "mirror"}]), None).unwrap();
        let targets = get(&router, "/mirror-targets/default").await;
        assert_eq!(targets["targets"][0]["reachable"], false);
    }

    #[tokio::test]
    async fn unsafe_targets_and_bad_requests_are_400() {
        let (dir, _cache, router, _s, _ids) = sample();
        let (status, body) = call_text(&router, "/mirror-library/default", json!({"target": dir.path().join("inside").to_str().unwrap(), "mode": "mirror"})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("inside the library"), "the reason is the response body: {body}");
        let out = tempfile::tempdir().unwrap();
        std::fs::write(out.path().join("keep.txt"), "mine").unwrap();
        let (status, body) = call_text(&router, "/mirror-library/default", json!({"target": out.path().to_str().unwrap(), "mode": "travel"})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(body.contains("not empty"), "{body}");
        let (status, _) = call(&router, "POST", "/mirror-library/default", json!({"target": "relative", "mode": "travel"})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = call(&router, "POST", "/mirror-library/default", json!({"target": out.path().to_str().unwrap(), "mode": "sideways"})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = call(&router, "POST", "/mirror-library/default", json!({"target": out.path().join("x").to_str().unwrap(), "mode": "mirror", "scope": {"book_ids": [1]}})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "a mirror is never a subset");
        let (status, _) = call(&router, "POST", "/mirror-library/default", json!({"target": out.path().join("x").to_str().unwrap(), "mode": "travel", "scope": {"book_ids": [1], "folder_prefix": "a"}})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "one scope at a time");
    }

    #[tokio::test]
    async fn a_travel_subset_job_copies_only_the_chosen_books() {
        let (_dir, _cache, router, _s, ids) = sample();
        let out = tempfile::tempdir().unwrap();
        let target = out.path().join("laptop");
        let req = json!({"target": target.to_str().unwrap(), "mode": "travel", "scope": {"book_ids": [ids[0]]}});
        let (status, dry) = call(&router, "POST", "/mirror-library/default", req.clone()).await;
        assert_eq!(status, StatusCode::OK, "{dry}");
        assert_eq!(dry["books"], 1);
        let mut run = req.clone();
        run["dry_run"] = json!(false);
        run["fingerprint"] = dry["fingerprint"].clone();
        let (status, started) = call(&router, "POST", "/mirror-library/default", run).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{started}");
        let job = finished(&router, &started).await;
        assert_eq!(job["report"]["complete"], true, "{job}");
        let copy = Cache::new(&target).unwrap();
        assert_eq!(copy.all_book_ids().unwrap(), vec![ids[0]]);
        let (_, targets) = (0, get(&router, "/mirror-targets/default").await);
        assert_eq!(targets["targets"][0]["scope"], json!({"book_ids": [ids[0]]}));
    }
}
