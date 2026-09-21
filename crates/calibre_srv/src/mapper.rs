//! `POST /mapper/preview` and `POST /mapper/apply` -- real, **new**
//! routes (issue 1.7 of the #816 epic), not a port: upstream applies
//! author/tag mapping from its Qt preferences dialogs, so there is no
//! `calibre.srv` endpoint to port here.
//!
//! # The engines already existed
//!
//! `calibre_ebooks::metadata::author_mapper` and `::tag_mapper` are
//! full ports, with real rule compilation, ICU-aware case folding and
//! regex matching. Nothing called them. This is the usual shape for
//! this project: the work is reaching a merged engine, not writing
//! one.
//!
//! # Preview is the point
//!
//! A mapping rule applied across a whole library is not undoable --
//! there is no record of what an author was called before the rule
//! rewrote it. So `preview` runs exactly the same computation as
//! `apply` and writes nothing, returning only the books that would
//! change. The UI is expected to show that before offering the button
//! that writes.
//!
//! # Scope
//!
//! `authors` and `tags`, matching the two engines that exist.
//! Upstream also ships publisher and series mappers; those have no
//! ported engine here, and asking for one 400s with a clear message
//! rather than silently doing nothing.

use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::Json;
use calibre_ebooks::metadata::{author_mapper, tag_mapper};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::errors::ServerError;
use crate::AppState;

/// One mapping rule, in the shape both engines understand.
///
/// `action` and `match_type` are validated rather than passed
/// through: `author_mapper::matcher` falls back to a never-matching
/// closure for an unknown `match_type`, and `tag_mapper` defaults an
/// unknown `action` to `keep`. Both are silent no-ops, which is the
/// worst outcome for a rule the user believes they wrote correctly.
#[derive(Debug, Clone, Deserialize)]
pub struct MapperRule {
    pub action: String,
    pub query: String,
    #[serde(default)]
    pub replace: Option<String>,
    pub match_type: String,
}

const MATCH_TYPES: &[&str] = &["one_of", "not_one_of", "matches", "not_matches", "has"];
const AUTHOR_ACTIONS: &[&str] = &["replace"];
const TAG_ACTIONS: &[&str] = &["keep", "remove", "replace"];

#[derive(Debug, Deserialize)]
pub struct MapperBody {
    /// `authors` or `tags`.
    pub field: String,
    pub rules: Vec<MapperRule>,
    /// Books to act on. Omitted or empty means the whole library.
    #[serde(default)]
    pub book_ids: Vec<i32>,
}

#[derive(Debug, Serialize)]
pub struct MappedBook {
    pub book_id: i32,
    pub title: String,
    pub before: Vec<String>,
    pub after: Vec<String>,
}

fn validate(field: &str, rules: &[MapperRule]) -> Result<(), ServerError> {
    if field != "authors" && field != "tags" {
        return Err(ServerError::BadRequest(format!(
            "field must be one of: authors, tags (got {field:?}). Publisher and series mappers exist upstream but have no ported engine here."
        )));
    }
    if rules.is_empty() {
        return Err(ServerError::BadRequest("rules must not be empty".to_string()));
    }

    let allowed_actions = if field == "authors" { AUTHOR_ACTIONS } else { TAG_ACTIONS };
    for (i, rule) in rules.iter().enumerate() {
        if !MATCH_TYPES.contains(&rule.match_type.as_str()) {
            return Err(ServerError::BadRequest(format!("rule {i}: match_type must be one of {MATCH_TYPES:?} (got {:?})", rule.match_type)));
        }
        if !allowed_actions.contains(&rule.action.as_str()) {
            return Err(ServerError::BadRequest(format!("rule {i}: action must be one of {allowed_actions:?} for {field} (got {:?})", rule.action)));
        }
        if rule.action == "replace" && rule.replace.is_none() {
            return Err(ServerError::BadRequest(format!("rule {i}: action \"replace\" needs a replace value")));
        }
        // A pattern that does not compile would otherwise degrade to a
        // matcher that never fires -- a rule silently doing nothing.
        if rule.match_type.contains("matches") {
            if let Err(e) = regex::Regex::new(&rule.query) {
                return Err(ServerError::BadRequest(format!("rule {i}: invalid pattern {:?}: {e}", rule.query)));
            }
        }
    }
    Ok(())
}

/// Runs the rules over one book's values.
fn map_values(field: &str, values: Vec<String>, rules: &[MapperRule]) -> Vec<String> {
    if field == "authors" {
        let engine_rules: Vec<author_mapper::Rule> = rules
            .iter()
            .map(|r| author_mapper::Rule { action: r.action.clone(), query: r.query.clone(), replace: r.replace.clone(), match_type: r.match_type.clone() })
            .collect();
        let compiled = author_mapper::compile_rules(&engine_rules);
        author_mapper::map_authors(&values, &compiled)
    } else {
        let engine_rules: Vec<HashMap<String, String>> = rules
            .iter()
            .map(|r| {
                let mut m = HashMap::new();
                m.insert("action".to_string(), r.action.clone());
                m.insert("query".to_string(), r.query.clone());
                m.insert("match_type".to_string(), r.match_type.clone());
                if let Some(replace) = &r.replace {
                    m.insert("replace".to_string(), replace.clone());
                }
                m
            })
            .collect();
        tag_mapper::map_tags(values, engine_rules, None)
    }
}

/// Reads a multi-valued field off a raw `get_data_as_dict` row.
///
/// The shapes are not consistent between fields, which is a real trap:
/// `tags` and `languages` come back as JSON arrays, but `authors`
/// comes back as a single `&`-joined **string**. `/ajax/book` hides
/// this because `ajax::book_json` normalizes authors into an array
/// first -- so testing against that endpoint, or reasoning from it,
/// suggests a uniformity the raw row does not have.
///
/// Handling only the array case makes author mapping silently match
/// nothing, which is exactly how this was first written.
fn read_values(row: &Value, field: &str) -> Vec<String> {
    match row.get(field) {
        Some(Value::Array(items)) => items.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
        Some(Value::String(joined)) => {
            let sep = if field == "authors" { '&' } else { ',' };
            joined.split(sep).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
        }
        _ => Vec::new(),
    }
}

/// Every book the rules would change, with its before and after.
async fn compute(state: &AppState, library_id: Option<&str>, body: &MapperBody) -> Result<Vec<MappedBook>, ServerError> {
    validate(&body.field, &body.rules)?;

    let cache = state.cache_for(library_id).ok_or_else(|| ServerError::NotFound(format!("no library named {:?}", library_id.unwrap_or(""))))?;
    let field = body.field.clone();
    let rules = body.rules.clone();
    let requested: Vec<i32> = body.book_ids.clone();

    tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<MappedBook>> {
        let ids = if requested.is_empty() { cache.all_book_ids()? } else { requested };
        let id_set: std::collections::HashSet<i32> = ids.iter().copied().collect();
        let rows = cache.get_data_as_dict(None, true, Some(&id_set), false)?;

        let mut out = Vec::new();
        for row in rows {
            let Some(book_id) = row.get("id").and_then(|v| v.as_i64()).map(|v| v as i32) else { continue };
            let before = read_values(&row, &field);
            if before.is_empty() {
                continue;
            }
            let after = map_values(&field, before.clone(), &rules);
            if after != before {
                let title = row.get("title").and_then(|v| v.as_str()).unwrap_or("Unknown").to_string();
                out.push(MappedBook { book_id, title, before, after });
            }
        }
        Ok(out)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))
}

async fn preview_handle(state: AppState, library_id: Option<&str>, body: MapperBody) -> Result<Json<Value>, ServerError> {
    let changed = compute(&state, library_id, &body).await?;
    Ok(Json(json!({ "changed": changed.len(), "books": changed })))
}

async fn apply_handle(state: AppState, library_id: Option<&str>, body: MapperBody) -> Result<Json<Value>, ServerError> {
    let changed = compute(&state, library_id, &body).await?;
    let cache = state.cache_for(library_id).ok_or_else(|| ServerError::NotFound(format!("no library named {:?}", library_id.unwrap_or(""))))?;

    // `set_field` takes the joined text for multi-valued fields, with
    // the same separators `set_many_to_many_field` splits on.
    let field = body.field.clone();
    let writes: Vec<(i32, String)> = changed.iter().map(|b| (b.book_id, b.after.join(if field == "authors" { " & " } else { ", " }))).collect();

    let written = tokio::task::spawn_blocking(move || -> anyhow::Result<usize> {
        let mut n = 0;
        for (book_id, value) in writes {
            cache.set_field(book_id, &field, &value)?;
            n += 1;
        }
        Ok(n)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    Ok(Json(json!({ "changed": written, "books": changed })))
}

pub async fn preview(State(state): State<AppState>, Json(body): Json<MapperBody>) -> Result<Json<Value>, ServerError> {
    preview_handle(state, None, body).await
}

pub async fn preview_for_library(State(state): State<AppState>, Path(library_id): Path<String>, Json(body): Json<MapperBody>) -> Result<Json<Value>, ServerError> {
    preview_handle(state, Some(&library_id), body).await
}

pub async fn apply(State(state): State<AppState>, Json(body): Json<MapperBody>) -> Result<Json<Value>, ServerError> {
    apply_handle(state, None, body).await
}

pub async fn apply_for_library(State(state): State<AppState>, Path(library_id): Path<String>, Json(body): Json<MapperBody>) -> Result<Json<Value>, ServerError> {
    apply_handle(state, Some(&library_id), body).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use calibre_db::cache::Cache;
    use tower::ServiceExt;

    fn app_with_books(books: &[(&str, &str)]) -> (tempfile::TempDir, axum::Router) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        for (title, author) in books {
            let path = dir.path().join(format!("{title}.epub"));
            std::fs::write(&path, b"fake epub bytes").unwrap();
            let mut meta = calibre_ebooks::metadata::MetaInformation::default();
            meta.title = title.to_string();
            meta.authors = vec![author.to_string()];
            cache.add_book(&path, &meta).unwrap();
        }
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
        let router = crate::test_router(state);
        (dir, router)
    }

    /// Returns the raw body text alongside the parsed JSON: error
    /// responses are plain text, so asserting on their content via
    /// `Value` would only ever see `null`.
    async fn post(router: &axum::Router, uri: &str, body: Value) -> (StatusCode, Value, String) {
        let req = Request::builder().method("POST").uri(uri).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null), text)
    }

    async fn get(router: &axum::Router, uri: &str) -> Value {
        let resp = router.clone().oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap()).await.unwrap();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    }

    fn replace_rule(query: &str, replace: &str) -> Value {
        json!({"action": "replace", "query": query, "replace": replace, "match_type": "one_of"})
    }

    /// The shape trap, pinned directly. `get_data_as_dict` returns
    /// `tags` as an array but `authors` as an `&`-joined string, and
    /// `/ajax/book` hides that because `book_json` normalizes authors
    /// first. Reading only the array case makes author mapping match
    /// nothing at all -- which is how this was first written, and the
    /// route-level tests are slow enough to make the cause unobvious.
    #[test]
    fn read_values_handles_both_row_shapes() {
        let row = json!({ "authors": "Ann Lee & Bo Fox", "tags": ["a", "b"], "publisher": null });

        assert_eq!(read_values(&row, "authors"), vec!["Ann Lee", "Bo Fox"], "the &-joined string form must be split");
        assert_eq!(read_values(&row, "tags"), vec!["a", "b"], "the array form must pass through");
        assert_eq!(read_values(&row, "publisher"), Vec::<String>::new());
        assert_eq!(read_values(&row, "missing"), Vec::<String>::new());
    }

    #[test]
    fn read_values_drops_empties_from_a_joined_string() {
        let row = json!({ "authors": "Ann Lee &  & Bo Fox" });
        assert_eq!(read_values(&row, "authors"), vec!["Ann Lee", "Bo Fox"]);
    }

    #[tokio::test]
    async fn preview_reports_what_would_change_for_authors() {
        let (_d, router) = app_with_books(&[("A", "Jane Doe"), ("B", "Someone Else")]);

        let (status, body, text) = post(&router, "/mapper/preview", json!({"field": "authors", "rules": [replace_rule("Jane Doe", "J. Doe")]})).await;

        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["changed"], 1, "only the matching book should be listed: {body}");
        assert_eq!(body["books"][0]["before"], json!(["Jane Doe"]));
        assert_eq!(body["books"][0]["after"], json!(["J. Doe"]));
    }

    /// The whole reason preview exists: a mapping rule applied across a
    /// library is not undoable, so previewing must not write.
    #[tokio::test]
    async fn preview_writes_nothing() {
        let (_d, router) = app_with_books(&[("A", "Jane Doe")]);

        post(&router, "/mapper/preview", json!({"field": "authors", "rules": [replace_rule("Jane Doe", "J. Doe")]})).await;

        let book = get(&router, "/ajax/book/1").await;
        assert_eq!(book["authors"], json!(["Jane Doe"]), "preview must not modify the library");
    }

    #[tokio::test]
    async fn apply_really_writes_the_mapped_value() {
        let (_d, router) = app_with_books(&[("A", "Jane Doe")]);

        let (status, body, text) = post(&router, "/mapper/apply", json!({"field": "authors", "rules": [replace_rule("Jane Doe", "J. Doe")]})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["changed"], 1);

        let book = get(&router, "/ajax/book/1").await;
        assert_eq!(book["authors"], json!(["J. Doe"]));
    }

    #[tokio::test]
    async fn book_ids_scope_the_edit_to_a_selection() {
        let (_d, router) = app_with_books(&[("A", "Jane Doe"), ("B", "Jane Doe")]);

        post(&router, "/mapper/apply", json!({"field": "authors", "rules": [replace_rule("Jane Doe", "J. Doe")], "book_ids": [1]})).await;

        assert_eq!(get(&router, "/ajax/book/1").await["authors"], json!(["J. Doe"]));
        assert_eq!(get(&router, "/ajax/book/2").await["authors"], json!(["Jane Doe"]), "a book outside the selection must be untouched");
    }

    #[tokio::test]
    async fn a_rule_that_matches_nothing_changes_nothing() {
        let (_d, router) = app_with_books(&[("A", "Jane Doe")]);

        let (_, body, _) = post(&router, "/mapper/preview", json!({"field": "authors", "rules": [replace_rule("Nobody At All", "X")]})).await;
        assert_eq!(body["changed"], 0);
    }

    #[tokio::test]
    async fn tags_can_be_removed_by_rule() {
        let (_d, router) = app_with_books(&[("A", "Jane Doe")]);
        post(&router, "/cdb/set-fields/1", json!({"changes": {"tags": ["keepme", "dropme"]}})).await;

        let (status, body, text) = post(&router, "/mapper/apply", json!({"field": "tags", "rules": [{"action": "remove", "query": "dropme", "match_type": "one_of"}]})).await;
        assert_eq!(status, StatusCode::OK, "{body}");

        assert_eq!(get(&router, "/ajax/book/1").await["tags"], json!(["keepme"]));
    }

    // `author_mapper::matcher` falls back to a closure that never
    // matches for an unknown match_type, and `tag_mapper` defaults an
    // unknown action to "keep". Both are silent no-ops -- the worst
    // outcome for a rule the user believes they wrote correctly.
    #[tokio::test]
    async fn an_unknown_match_type_is_refused_rather_than_silently_ignored() {
        let (_d, router) = app_with_books(&[("A", "Jane Doe")]);

        let (status, _, _) = post(&router, "/mapper/preview", json!({"field": "authors", "rules": [{"action": "replace", "query": "x", "replace": "y", "match_type": "sounds_like"}]})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn an_action_the_field_does_not_support_is_refused() {
        let (_d, router) = app_with_books(&[("A", "Jane Doe")]);

        // "remove" is a tag action; the author engine only replaces.
        let (status, _, _) = post(&router, "/mapper/preview", json!({"field": "authors", "rules": [{"action": "remove", "query": "x", "match_type": "one_of"}]})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    /// An uncompilable pattern degrades to a never-matching matcher
    /// inside the engine, so it has to be caught here.
    #[tokio::test]
    async fn an_invalid_regex_is_refused_up_front() {
        let (_d, router) = app_with_books(&[("A", "Jane Doe")]);

        let (status, body, text) = post(&router, "/mapper/preview", json!({"field": "authors", "rules": [{"action": "replace", "query": "(unclosed", "replace": "y", "match_type": "matches"}]})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(text.contains("invalid pattern"), "{text}");
    }

    #[tokio::test]
    async fn replace_without_a_replacement_is_refused() {
        let (_d, router) = app_with_books(&[("A", "Jane Doe")]);

        let (status, _, _) = post(&router, "/mapper/preview", json!({"field": "authors", "rules": [{"action": "replace", "query": "x", "match_type": "one_of"}]})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn an_unsupported_field_is_refused_with_a_clear_message() {
        let (_d, router) = app_with_books(&[("A", "Jane Doe")]);

        let (status, body, text) = post(&router, "/mapper/preview", json!({"field": "publisher", "rules": [replace_rule("a", "b")]})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(text.contains("authors, tags"), "{text}");
    }

    #[tokio::test]
    async fn an_empty_rule_list_is_refused() {
        let (_d, router) = app_with_books(&[("A", "Jane Doe")]);

        let (status, _, _) = post(&router, "/mapper/preview", json!({"field": "authors", "rules": []})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}
