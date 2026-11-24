//! Port of a subset of `calibre.srv.ajax` -- the read-only JSON REST
//! API that calibre's own web-reader/book-list JS client (and any
//! other JSON-consuming client) uses to browse a library, as an
//! alternative to the Atom/OPDS feeds in [`crate::opds`].
//!
//! # Disclosed simplifications
//!
//! Upstream's `book_to_json`/`categories`/`category` are driven by a
//! full `field_metadata` system (custom columns, hierarchical
//! categories, user categories, per-field `is_category`/`is_csp`
//! introspection) this crate doesn't have (see
//! `lexmechanic_db::categories`'s own doc for the same narrowing already
//! disclosed for the OPDS category feeds). This port covers:
//!
//! - `GET /ajax/book/{book_id}` / `GET /ajax/books?ids=...`: real book
//!   metadata as JSON, built from `Cache::get_data_as_dict`. No
//!   `category_urls` cross-referencing, no `id_is_uuid`, no
//!   `device_compatible`/`device_for_template` (device-specific upload
//!   path templating) -- all upstream features this crate's narrower
//!   data model can't drive yet.
//! - `GET /ajax/categories`: the same five standard categories
//!   `lexmechanic_db::categories` supports. No icon URLs (no icon assets
//!   are part of this port), no "All books"/"Newest" pseudo-entries.
//! - `GET /ajax/category/{name}`: flat items only -- no subcategories,
//!   no hierarchical/user categories.
//! - `GET /ajax/books_in/{category}/{item}`: book ids for one category
//!   item, single-field sort only (upstream supports a comma-separated
//!   multi-field sort), no `get_additional_fields`.
//! - `GET /ajax/search`: same query language as `opds::search`, JSON
//!   shape instead of an Atom feed. Now supports `vl=` (issue #500),
//!   via [`lexmechanic_db::search::books_in_virtual_library`], and a real
//!   comma-separated multi-field `sort` (issue #759) -- unlike
//!   `books_in` above, which stays single-field-only.
//! - `GET /ajax/library-info`: single-library only.
//!
//! # New in issue #500 (part of the #432 browser-UI epic)
//!
//! `old_src/src/pyj/book_list/` (the library-browser client this
//! `/ajax/*` API is being extended for) calls upstream's own
//! `interface-data/*` endpoint family, which doesn't exist in this
//! crate -- see #500's own issue body for why extending `/ajax/*`
//! (rather than porting `interface-data/*` verbatim) was chosen. The
//! pieces it actually needs, added here:
//!
//! - `GET /ajax/field-metadata`: [`lexmechanic_db::field_metadata::FieldMetadata::all_metadata`]
//!   + `ui_sortable_field_keys` in one response, replacing upstream's
//!   `db.field_metadata.all_metadata()`/`sortable_fields` (computed
//!   inline as part of `get_library_init_data` upstream, a standalone
//!   endpoint here instead since this crate has no combined
//!   library-init endpoint to fold it into).
//! - `GET /ajax/virtual-libraries`: the whole `{name: query}` map
//!   (`lexmechanic_db::cache::Cache::virtual_library_map`), matching
//!   upstream's own `db._pref('virtual_libraries', {})`.
//! - `GET`/`POST /ajax/session-data`: per-user client-prefs storage,
//!   port of `interface-data/set-session-data` (a GET half added too
//!   -- upstream only exposes reading session data bundled inside
//!   `interface-data/init`, which this crate doesn't have). Backed by
//!   `lexmechanic_srv::users::UserManager`'s `session_data` column,
//!   reachable only via [`AppState::auth`] -- when the server is run
//!   without `--auth` (`AppState::auth` is `None`), there is no user
//!   store at all to persist into, so `GET` returns `{}` and `POST` is
//!   a silent no-op, same effective behavior as upstream's own
//!   anonymous-user no-op (`if rd.username: ...`) even though the
//!   *reason* differs (no auth mode vs. no logged-in user).

use std::collections::HashSet;

use axum::extract::{Extension, Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::Value;

use lexmechanic_db::categories;

use crate::auth::AuthenticatedUser;
use crate::books::effective_user;
use crate::errors::ServerError;
use crate::opds::sort_key_for;
use crate::AppState;

/// The one library id this single-library server exposes -- matches
/// `content.rs`'s own `"default"` convention (see
/// `ServerError::book_not_found`'s call sites).
const LIBRARY_ID: &str = "default";

fn get_pagination(num: Option<i64>, offset: Option<i64>) -> Result<(i64, i64), ServerError> {
    let num = num.unwrap_or(100);
    let offset = offset.unwrap_or(0);
    if num < 0 {
        return Err(ServerError::NotFound("Invalid num".to_string()));
    }
    if offset < 0 {
        return Err(ServerError::NotFound("Invalid offset".to_string()));
    }
    Ok((num, offset))
}

/// Port of `book_to_json`, narrowed per this module's doc. Reshapes
/// one `Cache::get_data_as_dict` row into the JSON API's own shape:
/// drops internal `fmt_<ext>` absolute paths in favor of `/get/...`
/// URLs (matching `opds::acquisition_entry`'s own URL scheme), and
/// halves `rating` (0..10 storage -> 0..5 display), matching
/// upstream's non-`device_compatible` path.
pub(crate) fn book_json(row: &Value) -> Value {
    let book_id = row["id"].as_i64().unwrap_or(0);
    let mut out = row.as_object().cloned().unwrap_or_default();
    out.remove("size");

    let rating = row["rating"].as_f64().map(|r| r / 2.0);
    out.insert("rating".into(), serde_json::json!(rating));

    out.insert("cover".into(), serde_json::json!(format!("/get/cover/{book_id}")));
    out.insert("thumbnail".into(), serde_json::json!(format!("/get/thumb/{book_id}")));

    let mut formats: Vec<String> = row["available_formats"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).map(str::to_lowercase).collect()).unwrap_or_default();
    formats.sort();
    out.remove("formats");
    for fmt in &formats {
        out.remove(&format!("fmt_{fmt}"));
    }
    let mut other_formats = serde_json::Map::new();
    let main_format = formats.first().map(|fmt| {
        let url = format!("/get/{fmt}/{book_id}");
        for other in formats.iter().skip(1) {
            other_formats.insert(other.clone(), serde_json::json!(format!("/get/{other}/{book_id}")));
        }
        serde_json::json!({ fmt.clone(): url })
    });
    out.insert("formats".into(), serde_json::json!(formats));
    out.insert("main_format".into(), main_format.unwrap_or(Value::Null));
    out.insert("other_formats".into(), Value::Object(other_formats));

    Value::Object(out)
}

pub(crate) async fn fetch_rows(state: &AppState, ids: HashSet<i32>) -> Result<Vec<Value>, ServerError> {
    fetch_rows_from_cache(state.cache.clone(), ids).await
}

/// Same as [`fetch_rows`], against an already-resolved cache -- for a
/// caller (e.g. `cdb::set_fields`, issue #500) that resolved its own
/// `Cache` via [`AppState::cache_for`] rather than always using the
/// default [`AppState::cache`].
pub(crate) async fn fetch_rows_from_cache(cache: std::sync::Arc<lexmechanic_db::cache::Cache>, ids: HashSet<i32>) -> Result<Vec<Value>, ServerError> {
    tokio::task::spawn_blocking(move || cache.get_data_as_dict_unchecked(None, false, Some(&ids), false))
        .await
        .map_err(|e| ServerError::InternalServerError(e.to_string()))?
        .map_err(|e| ServerError::InternalServerError(e.to_string()))
}

/// Fields [`sort_row`] can build without a full [`fetch_rows`] row.
const FAST_SORT_FIELDS: &[&str] = &[
    "title", "sort", "authors", "author_sort", "publisher", "series", "uuid", "comments", "isbn", "identifiers",
    "rating", "size", "series_index", "tags", "languages", "timestamp", "pubdate", "last_modified", "id",
    "folder", "file_name",
];

/// Fields whose values are numbers: compared as numbers, not as the text of
/// the number (which would put 9 after 10).
const NUMERIC_SORT_FIELDS: &[&str] = &["size", "rating", "series_index", "id"];

/// A string that sorts, character by character, in the same order as the
/// number does: zero-padded to a fixed width, with negatives (never seen in
/// practice) sorting before everything else.
fn numeric_key(n: f64) -> String {
    if n.is_nan() {
        String::new()
    } else if n < 0.0 {
        format!("-{:020.6}", (1e13 + n).max(0.0))
    } else {
        format!("0{:020.6}", n)
    }
}

/// The sort key of `field` for one row: numbers as padded text, the
/// file-oriented fields from `files`, everything else via [`sort_key_for`].
fn key_for(row: &Value, field: &str, files: &FileSortData) -> String {
    let id = row["id"].as_i64().unwrap_or(0) as i32;
    match field {
        "folder" => files.folders.get(&id).cloned().unwrap_or_default(),
        "file_name" => files.names.get(&id).cloned().unwrap_or_default(),
        f if NUMERIC_SORT_FIELDS.contains(&f) => match &row[f] {
            Value::Number(n) => n.as_f64().map(numeric_key).unwrap_or_default(),
            _ => String::new(),
        },
        f => sort_key_for(row, f),
    }
}

/// What the file-oriented sort fields need beyond a row: the primary file
/// names (one bulk query) and the folders (in memory), loaded only when the
/// sort asks for them.
#[derive(Default)]
struct FileSortData {
    names: std::collections::HashMap<i32, String>,
    folders: std::collections::HashMap<i32, String>,
}

impl FileSortData {
    fn load(cache: &lexmechanic_db::cache::Cache, ids: &HashSet<i32>, fields: &[String]) -> anyhow::Result<Self> {
        let mut data = Self::default();
        if fields.iter().any(|f| f == "file_name") {
            data.names = cache.primary_file_names(None)?;
        }
        if fields.iter().any(|f| f == "folder") {
            let store = cache.fields()?;
            for id in ids {
                data.folders.insert(*id, store.field_for(*id, "path").unwrap_or_default());
            }
        }
        Ok(data)
    }
}

/// Each matching book's id and its sort keys, one key per sort field.
///
/// The keys are exactly what [`sort_key_for`] makes of a [`fetch_rows`]
/// row, but for the usual fields they are built from only those fields:
/// a full row also reads the cover, every format and every custom column
/// with queries of their own, which is what made sorting 9,767 books take
/// about a second. Any other field (a custom column, say) uses full rows
/// as before. Books without a title are left out, as `get_data_as_dict`
/// leaves them out.
async fn sort_keys(cache: &std::sync::Arc<lexmechanic_db::cache::Cache>, ids: HashSet<i32>, fields: &[&str]) -> Result<Vec<(i64, Vec<String>)>, ServerError> {
    let fast = fields.iter().all(|f| FAST_SORT_FIELDS.contains(f));
    let cache = cache.clone();
    let fields: Vec<String> = fields.iter().map(|f| f.to_string()).collect();
    tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<(i64, Vec<String>)>> {
        let files = FileSortData::load(&cache, &ids, &fields)?;
        if !fast {
            // Only a sort on `formats` reads what is on disk.
            let rows = if fields.iter().any(|f| f == "formats") { cache.get_data_as_dict(None, false, Some(&ids), false)? } else { cache.get_data_as_dict_unchecked(None, false, Some(&ids), false)? };
            return Ok(rows.iter().map(|r| (r["id"].as_i64().unwrap_or(0), fields.iter().map(|f| key_for(r, f, &files)).collect())).collect());
        }
        let mut ids: Vec<i32> = ids.into_iter().collect();
        ids.sort_unstable();
        let store = cache.fields()?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(row) = sort_row(&store, id, &fields)? {
                out.push((id as i64, fields.iter().map(|f| key_for(&row, f, &files)).collect()));
            }
        }
        Ok(out)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))
}

/// Whether comparing these keys as bytes orders them exactly as ICU collation
/// would: every non-empty key has the same length and the same non-digit
/// characters in the same places, so they differ only in digits, which both
/// orders put in numeric order. That is the shape of the zero-padded numbers
/// and fixed-format dates and times these columns hold. Anything else (a date
/// with a different number of fractional digits, a sign) is left to ICU, where
/// a punctuation character can sort differently from its byte value.
fn bytewise_order_matches_icu<'a>(keys: impl Iterator<Item = &'a str>) -> bool {
    let mut template: Option<&str> = None;
    for key in keys {
        if key.is_empty() {
            continue;
        }
        match template {
            None => template = Some(key),
            Some(t) => {
                if t.len() != key.len() || !t.bytes().zip(key.bytes()).all(|(a, b)| a == b || (a.is_ascii_digit() && b.is_ascii_digit())) {
                    return false;
                }
            }
        }
    }
    true
}

/// Sorts `keyed` by its keys, field by field. A column whose keys are
/// numbers or dates of one fixed shape is compared as bytes (see
/// [`bytewise_order_matches_icu`]), the rest with ICU collation. The sort is
/// stable, so equal keys stay in the order they came in.
fn sort_keyed(keyed: &mut [(i64, Vec<String>)], fields: usize, descending: bool) {
    let bytewise = bytewise_columns(keyed, fields);
    keyed.sort_by(|(_, a), (_, b)| compare_keys(a, b, &bytewise, descending));
}

/// Which of the `fields` columns of `keyed` are compared as bytes (see [`bytewise_order_matches_icu`]).
fn bytewise_columns(keyed: &[(i64, Vec<String>)], fields: usize) -> Vec<bool> {
    (0..fields).map(|j| bytewise_order_matches_icu(keyed.iter().map(|(_, keys)| keys[j].as_str()))).collect()
}

/// The order of two rows' keys, field by field.
fn compare_keys(a: &[String], b: &[String], bytewise: &[bool], descending: bool) -> std::cmp::Ordering {
    for ((ka, kb), bytes) in a.iter().zip(b.iter()).zip(bytewise.iter()) {
        let (x, y) = if descending { (kb, ka) } else { (ka, kb) };
        let cmp = if *bytes { x.as_bytes().cmp(y.as_bytes()) } else { lexmechanic_utils::icu::strcmp(x, y) };
        if cmp != std::cmp::Ordering::Equal {
            return cmp;
        }
    }
    std::cmp::Ordering::Equal
}

/// A library's ids in ascending sort order, with where each run of rows whose
/// keys compare equal begins. Rows with equal keys stay in id order in a
/// stable sort whichever way it runs, so the *descending* order is the same
/// runs taken last to first, each still in its own order: it is not simply
/// this order reversed, which would put the ties in descending id order.
/// Keeping this one order serves both directions.
struct SortedOrder {
    ids: Vec<i64>,
    run_starts: Vec<usize>,
}

impl SortedOrder {
    /// Sorts `keyed` ascending, which must already be in id order.
    fn build(mut keyed: Vec<(i64, Vec<String>)>, fields: usize) -> Self {
        let bytewise = bytewise_columns(&keyed, fields);
        keyed.sort_by(|(_, a), (_, b)| compare_keys(a, b, &bytewise, false));
        let mut run_starts = Vec::new();
        for i in 0..keyed.len() {
            if i == 0 || compare_keys(&keyed[i - 1].1, &keyed[i].1, &bytewise, false) != std::cmp::Ordering::Equal {
                run_starts.push(i);
            }
        }
        SortedOrder { ids: keyed.into_iter().map(|(id, _)| id).collect(), run_starts }
    }

    /// The ids in the order asked for, those in `keep` only.
    fn filtered(&self, descending: bool, keep: &HashSet<i32>) -> Vec<i64> {
        let wanted = |id: &&i64| keep.contains(&(**id as i32));
        if !descending {
            return self.ids.iter().filter(wanted).copied().collect();
        }
        let mut out = Vec::with_capacity(keep.len());
        let mut end = self.ids.len();
        for &start in self.run_starts.iter().rev() {
            out.extend(self.ids[start..end].iter().filter(wanted).copied());
            end = start;
        }
        out
    }
}

/// The ids of `ids` in sort order. When the result is most of the library
/// the order of the *whole* library is built once per library version and
/// kept, and each search just filters it (a sub-sequence of a stable sort is
/// the stable sort of the sub-set), so changing the query or paging does not
/// sort again.
async fn sorted_ids(cache: &std::sync::Arc<lexmechanic_db::cache::Cache>, ids: HashSet<i32>, fields: &[&str], descending: bool) -> Result<Vec<i64>, ServerError> {
    let fast = fields.iter().all(|f| FAST_SORT_FIELDS.contains(f));
    if !fast {
        let mut keyed = sort_keys(cache, ids, fields).await?;
        sort_keyed(&mut keyed, fields.len(), descending);
        return Ok(keyed.into_iter().map(|(id, _)| id).collect());
    }
    let cache = cache.clone();
    let field_list: Vec<String> = fields.iter().map(|f| f.to_string()).collect();
    tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<i64>> {
        let store = cache.fields()?;
        let keys_of = |ids: HashSet<i32>| -> anyhow::Result<Vec<(i64, Vec<String>)>> {
            let files = FileSortData::load(&cache, &ids, &field_list)?;
            let mut ids: Vec<i32> = ids.into_iter().collect();
            ids.sort_unstable();
            let mut keyed = Vec::with_capacity(ids.len());
            for id in ids {
                if let Some(row) = sort_row(&store, id, &field_list)? {
                    keyed.push((id as i64, field_list.iter().map(|f| key_for(&row, f, &files)).collect::<Vec<String>>()));
                }
            }
            Ok(keyed)
        };
        // The order of the whole library is kept, ascending (descending is
        // taken from it, see `SortedOrder`), for as long as nothing the sort
        // reads has changed: an edit to a rating no longer throws away the
        // order of titles. A search that matches a small part of the library
        // is sorted on its own, unless the whole order is already kept.
        let field_refs: Vec<&str> = field_list.iter().map(String::as_str).collect();
        let version = store.fields_version(&field_refs);
        let key = format!("sort:{}", field_list.join(","));
        if ids.len() * 4 < store.book_count() {
            if let Some(everything) = cache.cached_if_current::<SortedOrder>(&key, version) {
                return Ok(everything.filtered(descending, &ids));
            }
            let mut keyed = keys_of(ids)?;
            sort_keyed(&mut keyed, field_list.len(), descending);
            return Ok(keyed.into_iter().map(|(id, _)| id).collect());
        }
        let everything = cache.cached_for_version(&key, version, || keys_of(cache.all_book_ids()?.into_iter().collect()).map(|keyed| SortedOrder::build(keyed, field_list.len())))?;
        Ok(everything.filtered(descending, &ids))
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))
}

/// The part of `get_data_as_dict`'s row that `fields` sort on, built the
/// same way (same field, same parsing), so [`sort_key_for`] gives the same
/// key. `None` for a book without a title.
fn sort_row(store: &lexmechanic_db::fields::FieldStore, id: i32, fields: &[String]) -> anyhow::Result<Option<Value>> {
    let Some(title) = store.field_for(id, "title") else { return Ok(None) };
    let mut row = serde_json::Map::new();
    row.insert("id".into(), serde_json::json!(id));
    row.insert("title".into(), serde_json::json!(title));
    for field in fields {
        // `sort_key_for` reads `sort` for title and `author_sort` for authors.
        let stored = match field.as_str() {
            "title" => "sort",
            "authors" => "author_sort",
            f => f,
        };
        let value = match stored {
            // The file-oriented fields are keyed by `key_for` from `FileSortData`.
            "id" | "folder" | "file_name" => continue,
            "rating" | "series_index" => serde_json::json!(store.field_for(id, stored).and_then(|s| s.parse::<f64>().ok())),
            "size" => serde_json::json!(store.field_for(id, stored).and_then(|s| s.parse::<i64>().ok())),
            "isbn" | "identifiers" => serde_json::json!(store.field_for(id, stored).unwrap_or_default()),
            "tags" => serde_json::json!(store.field_for(id, stored).map(|s| s.split(", ").map(|t| t.trim().to_string()).collect::<Vec<_>>()).unwrap_or_default()),
            "languages" => serde_json::json!(store.field_for(id, stored).map(|s| s.split(", ").map(str::to_string).collect::<Vec<_>>()).unwrap_or_default()),
            _ => serde_json::json!(store.field_for(id, stored)),
        };
        row.insert(stored.to_string(), value);
    }
    Ok(Some(Value::Object(row)))
}

/// `GET /ajax/book/{book_id}`. Port of `book`, `id_is_uuid` not
/// supported (see module doc).
pub async fn book(State(state): State<AppState>, Path(book_id): Path<String>) -> Result<Json<Value>, ServerError> {
    let Ok(book_id) = book_id.parse::<i32>() else {
        return Err(ServerError::book_not_found(0, LIBRARY_ID));
    };
    let rows = fetch_rows(&state, std::iter::once(book_id).collect()).await?;
    let row = rows.into_iter().next().ok_or_else(|| ServerError::book_not_found(book_id, LIBRARY_ID))?;
    let mut out = book_json(&row);
    // Each format's file stem (`data.name`), so a client can name the exact
    // file on disk. In a folder shared by several books the extension alone
    // does not say whose file is whose.
    let cache = state.cache.clone();
    let names = tokio::task::spawn_blocking(move || cache.format_file_names(book_id)).await.map_err(|e| ServerError::InternalServerError(e.to_string()))?.unwrap_or_default();
    let names: serde_json::Map<String, Value> = names.into_iter().map(|(fmt, stem)| (fmt.to_lowercase(), Value::String(stem))).collect();
    if let Some(map) = out.as_object_mut() {
        map.insert("format_file_names".into(), Value::Object(names));
    }
    Ok(Json(out))
}

/// How many books `/ajax/books` returns when no `ids` are given.
const MAX_BOOKS_WITHOUT_IDS: usize = 1000;

/// The `limit` lowest ids of `all`.
fn cap_all(all: &HashSet<i32>, limit: usize) -> HashSet<i32> {
    let mut sorted: Vec<i32> = all.iter().copied().collect();
    sorted.sort_unstable();
    sorted.truncate(limit);
    sorted.into_iter().collect()
}

#[cfg(test)]
mod cap_tests {
    use super::*;

    #[test]
    fn a_bare_books_request_is_capped_to_the_lowest_ids() {
        let all: HashSet<i32> = (1..=50).collect();
        let capped = cap_all(&all, 10);
        assert_eq!(capped, (1..=10).collect::<HashSet<i32>>());
        assert_eq!(cap_all(&all, 100).len(), 50, "a small library is returned whole");
    }
}

#[derive(Debug, Deserialize)]
pub struct BooksQuery {
    ids: Option<String>,
    /// `files=1` adds the file-oriented fields the web table's optional
    /// Size, Folder and File name columns show (see [`add_file_fields`]).
    /// Absent, the response keeps upstream's shape exactly.
    files: Option<String>,
}

/// Adds `size` (largest format, bytes), `folder` (the book's folder relative
/// to the library root) and `file_name` (primary format's `stem.ext`) to each
/// row of `rows`. All three come from in-memory tables and one `IN` query, with
/// no stat of the disk, so a page of 100 books costs the same on a NAS.
fn add_file_fields(cache: &lexmechanic_db::cache::Cache, rows: &mut [Value]) -> anyhow::Result<()> {
    let ids: Vec<i32> = rows.iter().filter_map(|r| r["id"].as_i64()).map(|i| i as i32).collect();
    let names = cache.primary_file_names(Some(&ids))?;
    let store = cache.fields()?;
    for row in rows.iter_mut() {
        let Some(id) = row["id"].as_i64().map(|i| i as i32) else { continue };
        let Some(map) = row.as_object_mut() else { continue };
        map.insert("size".into(), serde_json::json!(store.field_for(id, "size").and_then(|s| s.parse::<i64>().ok())));
        map.insert("folder".into(), serde_json::json!(store.field_for(id, "path").unwrap_or_default()));
        map.insert("file_name".into(), serde_json::json!(names.get(&id)));
    }
    Ok(())
}

/// `GET /ajax/books?ids=1,2,3` (or `ids=all`/omitted, capped at
/// [`MAX_BOOKS_WITHOUT_IDS`]). Port of
/// `books`. Ids that don't exist map to `null`, matching upstream's
/// own `ans[book_id] = None` for ids outside `allowed_book_ids`.
pub async fn books(State(state): State<AppState>, Query(q): Query<BooksQuery>) -> Result<Json<Value>, ServerError> {
    let requested: Option<HashSet<i32>> = match q.ids.as_deref() {
        None | Some("all") => None,
        Some(s) => {
            let mut set = HashSet::new();
            for part in s.split(',') {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                let Ok(id) = part.parse::<i32>() else {
                    return Err(ServerError::NotFound("ids must a comma separated list of integers".to_string()));
                };
                set.insert(id);
            }
            Some(set)
        }
    };

    let all_ids: HashSet<i32> = tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        move || cache.all_book_ids()
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .into_iter()
    .collect();

    // Asking for "every book" is capped: on a library of hundreds of thousands
    // a bare `/ajax/books` built the whole JSON for all of them. The web UI
    // always names its ids, so nothing relies on the uncapped form; a caller
    // that wants more pages through explicit `ids`.
    let ids = requested.unwrap_or_else(|| cap_all(&all_ids, MAX_BOOKS_WITHOUT_IDS));
    let found: HashSet<i32> = ids.intersection(&all_ids).copied().collect();
    let rows = fetch_rows(&state, found).await?;

    let mut ans = serde_json::Map::new();
    for id in &ids {
        ans.insert(id.to_string(), Value::Null);
    }
    let mut shaped: Vec<Value> = rows.iter().map(book_json).collect();
    if q.files.as_deref().is_some_and(|f| f != "0" && f != "false") {
        let cache = state.cache.clone();
        shaped = tokio::task::spawn_blocking(move || add_file_fields(&cache, &mut shaped).map(|_| shaped))
            .await
            .map_err(|e| ServerError::InternalServerError(e.to_string()))?
            .map_err(|e| ServerError::InternalServerError(e.to_string()))?;
    }
    for out in shaped {
        let id = out["id"].as_i64().unwrap_or(0);
        ans.insert(id.to_string(), out);
    }
    Ok(Json(Value::Object(ans)))
}

/// `#label` -> display name, for the custom columns that are categories.
fn custom_category_names(cache: &lexmechanic_db::cache::Cache) -> std::collections::HashMap<String, String> {
    cache
        .custom_column_label_map()
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, v)| v["normalized"].as_bool().unwrap_or(false))
        .map(|(label, v)| (format!("#{label}"), v["name"].as_str().unwrap_or(&label).to_string()))
        .collect()
}

/// `GET /ajax/categories`. Port of `categories`, narrowed per module
/// doc (no icons, no "All books"/"Newest" pseudo-entries).
pub async fn categories_list(State(state): State<AppState>) -> Result<Json<Value>, ServerError> {
    // Only which categories have items is needed, not the items.
    let keys = tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        move || categories::non_empty_category_keys(&cache)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    let mut ans: Vec<Value> = Vec::new();
    for &(key, name) in crate::opds::CATEGORY_NAMES {
        if !keys.iter().any(|k| k == key) {
            continue;
        }
        ans.push(serde_json::json!({
            "url": format!("/ajax/category/{key}"),
            "name": name,
            "is_category": true,
        }));
    }
    // A normalized custom column (text, series, ...) with values is a category
    // too, keyed `#label`, shown under the column's own name.
    let names = custom_category_names(&state.cache);
    for key in keys.iter().filter(|k| k.starts_with('#')) {
        ans.push(serde_json::json!({
            "url": format!("/ajax/category/{key}"),
            "name": names.get(key.as_str()).cloned().unwrap_or_else(|| key.clone()),
            "is_category": true,
        }));
    }
    Ok(Json(Value::Array(ans)))
}

#[derive(Debug, Deserialize)]
pub struct CategoryQuery {
    #[serde(default = "default_num")]
    num: i64,
    #[serde(default)]
    offset: i64,
    sort: Option<String>,
    sort_order: Option<String>,
}

fn default_num() -> i64 {
    100
}

fn ensure_val<'a>(v: Option<&'a str>, allowed: &[&'a str]) -> &'a str {
    match v {
        Some(x) if allowed.contains(&x) => x,
        _ => allowed[0],
    }
}

/// `GET /ajax/category/{name}`. Port of `category`, narrowed per
/// module doc: flat items only, no subcategories.
pub async fn category(State(state): State<AppState>, Path(name): Path<String>, Query(q): Query<CategoryQuery>) -> Result<Json<Value>, ServerError> {
    let (num, offset) = get_pagination(Some(q.num), Some(q.offset))?;
    let sort = ensure_val(q.sort.as_deref(), &["name", "rating", "popularity"]);
    let sort_order = ensure_val(q.sort_order.as_deref(), &["asc", "desc"]);

    let category_display_name: Option<String> = crate::opds::CATEGORY_NAMES.iter().find(|(k, _)| *k == name).map(|(_, n)| n.to_string()).or_else(|| custom_category_names(&state.cache).get(name.as_str()).cloned());
    let Some(category_display_name) = category_display_name else {
        return Err(ServerError::NotFound(format!("Category {name:?} not found")));
    };

    // Only the requested category is built (and a standard one is kept until
    // the library changes).
    let cats = tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        let sort = sort.to_string();
        let name = name.clone();
        move || categories::get_category(&cache, &name, &sort, None).map(Option::unwrap_or_default)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    let total_num = cats.len() as i64;
    let (skip, take) = (offset.max(0) as usize, num.max(0) as usize);
    let page: Vec<&categories::Tag> = if sort_order == "desc" { cats.iter().rev().skip(skip).take(take).collect() } else { cats.iter().skip(skip).take(take).collect() };

    let items: Vec<Value> = page
        .iter()
        .map(|tag| {
            serde_json::json!({
                "name": tag.name,
                "average_rating": tag.avg_rating,
                "count": tag.count,
                "url": format!("/ajax/books_in/{name}/{}", tag.id),
                "has_children": false,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "category_name": category_display_name,
        "base_url": format!("/ajax/category/{name}"),
        "total_num": total_num,
        "offset": offset,
        "num": items.len(),
        "sort": sort,
        "sort_order": sort_order,
        "subcategories": Value::Array(vec![]),
        "items": items,
    })))
}

#[derive(Debug, Deserialize)]
pub struct BooksInQuery {
    #[serde(default = "default_num")]
    num: i64,
    #[serde(default)]
    offset: i64,
    sort: Option<String>,
    sort_order: Option<String>,
}

/// `GET /ajax/books_in/{category}/{item_id}`. Port of `books_in`,
/// narrowed per module doc: `item_id` must be the numeric category
/// item id `lexmechanic_db::categories::Tag::id` (no `allbooks`/`newest`/
/// `search` pseudo-categories -- those are already covered by
/// `opds::navcatalog`'s `title`/`newest` and `ajax::search`), single
/// sort field, no `get_additional_fields`. In particular, a named
/// saved search (upstream's `dname == 'search'` case here) is just a
/// `search:name` query to [`search`] now that saved searches are real
/// (issue #422) -- nothing to add to *this* endpoint for that.
/// `GET /ajax/search?query=search%3Aname`.
pub async fn books_in(State(state): State<AppState>, Path((category, item_id)): Path<(String, String)>, Query(q): Query<BooksInQuery>) -> Result<Json<Value>, ServerError> {
    let (num, offset) = get_pagination(Some(q.num), Some(q.offset))?;
    let sort_field = q.sort.as_deref().unwrap_or("title");
    let sort_order = ensure_val(q.sort_order.as_deref(), &["asc", "desc"]);
    let Ok(item_id) = item_id.parse::<i32>() else {
        return Err(ServerError::NotFound(format!("Category id {item_id:?} not an integer")));
    };

    let ids = tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        let category = category.clone();
        move || categories::book_ids_for_category_item(&cache, &category, item_id)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|_| ServerError::NotFound(format!("Category {category:?} not found")))?;

    let mut rows = fetch_rows(&state, ids).await?;
    rows.sort_by(|a, b| {
        let ka = sort_key_for(a, sort_field);
        let kb = sort_key_for(b, sort_field);
        if sort_order == "asc" {
            lexmechanic_utils::icu::strcmp(&ka, &kb)
        } else {
            lexmechanic_utils::icu::strcmp(&kb, &ka)
        }
    });

    let total_num = rows.len() as i64;
    let page: Vec<i64> = rows.into_iter().skip(offset.max(0) as usize).take(num.max(0) as usize).map(|r| r["id"].as_i64().unwrap_or(0)).collect();

    Ok(Json(serde_json::json!({
        "total_num": total_num,
        "sort_order": sort_order,
        "offset": offset,
        "num": page.len(),
        "sort": sort_field,
        "base_url": format!("/ajax/books_in/{category}/{item_id}"),
        "book_ids": page,
    })))
}

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    query: Option<String>,
    #[serde(default = "default_num")]
    num: i64,
    #[serde(default)]
    offset: i64,
    sort: Option<String>,
    sort_order: Option<String>,
    /// Virtual library name (issue #500) -- restricts results to the
    /// intersection of `query` and the named virtual library's own
    /// stored query, matching upstream's own `vl` handling in
    /// `get_basic_query_data`.
    vl: Option<String>,
}

/// `GET /ajax/search?query=...`. Port of `search`/`search_result`.
/// `sort` accepts a real comma-separated multi-field list (issue
/// #759; matches upstream's own request shape) applied as a real
/// tie-break chain -- ties on the first field fall through to the
/// second, and so on. `sort_order` stays a single scalar applied to
/// every field uniformly rather than upstream's own real per-field
/// direction toggle -- a disclosed narrowing, not a silent mismatch;
/// picking a different direction per field is a separable follow-up
/// if it ever matters in practice. `vl=` (issue #500) restricts to a
/// named virtual library.
pub async fn search(State(state): State<AppState>, Query(q): Query<SearchQuery>) -> Result<Json<Value>, ServerError> {
    let (num, offset) = get_pagination(Some(q.num), Some(q.offset))?;
    let sort_fields: Vec<&str> = q.sort.as_deref().unwrap_or("title").split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
    let sort_fields = if sort_fields.is_empty() { vec!["title"] } else { sort_fields };
    let sort_order = ensure_val(q.sort_order.as_deref(), &["asc", "desc"]);
    let query = q.query.unwrap_or_default();
    let vl = q.vl.unwrap_or_default();

    let ids: HashSet<i32> = tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        let query = query.clone();
        let vl = vl.clone();
        move || {
            let restriction = if query.is_empty() { None } else { Some(query.as_str()) };
            lexmechanic_db::search::books_in_virtual_library(&cache, &vl, restriction)
        }
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|_| ServerError::NotFound(format!("Search: {query:?} not understood")))?
    .into_iter()
    .collect();

    // Each row's sort keys are built once, not once per comparison, and the
    // library's order for this sort is kept between requests.
    let ordered = sorted_ids(&state.cache, ids, &sort_fields, sort_order != "asc").await?;

    let total_num = ordered.len() as i64;
    let page: Vec<i64> = ordered.into_iter().skip(offset.max(0) as usize).take(num.max(0) as usize).collect();

    Ok(Json(serde_json::json!({
        "total_num": total_num,
        "sort_order": sort_order,
        "offset": offset,
        "num": page.len(),
        "sort": sort_fields.join(","),
        "base_url": "/ajax/search",
        "query": query,
        "vl": vl,
        "library_id": LIBRARY_ID,
        "book_ids": page,
    })))
}

/// `GET /ajax/library-info`. Port of `library_info`, single-library
/// only.
pub async fn library_info(State(state): State<AppState>) -> Json<Value> {
    let path = state.cache.backend.library_path.clone();
    // The folder's own name, which is what a user calls a library --
    // upstream does the same, and an absolute path is too long to be a
    // label. Falls back to the full path for a root or otherwise
    // nameless directory rather than showing an empty string.
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());

    // Every library this server actually hosts, not a single hardcoded
    // entry. The UI's library switcher reads this map, so with one entry
    // in it there was nothing to switch to.
    let (library_map, default_library) = match &state.libraries {
        Some(broker) => (
            broker.library_map().into_iter().map(|(id, display)| (id, Value::String(display))).collect::<serde_json::Map<String, Value>>(),
            broker.default_library_id().to_string(),
        ),
        // Single-library mode keeps the id it has always reported, so a
        // client that remembers `default` is not broken by this.
        None => (
            std::iter::once((LIBRARY_ID.to_string(), Value::String(name.clone()))).collect(),
            LIBRARY_ID.to_string(),
        ),
    };

    Json(serde_json::json!({
        "library_map": library_map,
        "default_library": default_library,
        // The real path, so the UI can say *which* library is open.
        // Before this the name was the hardcoded string "Lexmechanic
        // Library" and the path was not exposed at all, so nothing on
        // screen could tell one library from another.
        "library_path": path.to_string_lossy(),
        // Where the library lives and what that implies (#258): the
        // storage tier (`network` for an SMB/NFS share), the SQLite
        // journal mode in effect (`delete` on a network tier, never
        // WAL), and any signals that another program has the library
        // open. The web UI shows a banner from this.
        "storage": state.cache.backend.storage_status(),
        // Whether the change log has missed changes (see `change_log_status`).
        "change_log": crate::change_log_status::status_json(&state.cache),
    }))
}

/// `GET /ajax/field-metadata` (issue #500). Port of the
/// `field_metadata`/`sortable_fields` pieces of upstream's
/// `get_library_init_data` -- see this module's own doc.
pub async fn field_metadata(State(state): State<AppState>) -> Result<Json<Value>, ServerError> {
    let (all_metadata, sortable_fields) = tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        move || -> anyhow::Result<(Value, Vec<(String, String)>)> {
            let fm = lexmechanic_db::field_metadata::FieldMetadata::from_cache(&cache)?;
            let sortable = fm.ui_sortable_field_keys().into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            Ok((fm.all_metadata(), sortable))
        }
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

    Ok(Json(serde_json::json!({
        "field_metadata": all_metadata,
        "sortable_fields": sortable_fields,
    })))
}

/// `GET /ajax/virtual-libraries` (issue #500). Port of upstream's own
/// `db._pref('virtual_libraries', {})` -- the whole `{name: query}`
/// map, unsorted (a client that wants a sorted name list can sort the
/// object's own keys; upstream's `ui` layer does the same).
pub async fn virtual_libraries(State(state): State<AppState>) -> Result<Json<Value>, ServerError> {
    let map = tokio::task::spawn_blocking({
        let cache = state.cache.clone();
        move || cache.virtual_library_map()
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?;
    Ok(Json(serde_json::json!(map)))
}

/// `GET /ajax/session-data` (issue #500). See this module's own doc
/// for the "no `--auth`, no user store" narrowing.
pub async fn get_session_data(State(state): State<AppState>, user: Option<Extension<AuthenticatedUser>>) -> Json<Value> {
    let username = effective_user(user);
    let data = state.auth.as_ref().map(|gate| gate.users.get_session_data(&username)).unwrap_or_else(|| serde_json::json!({}));
    Json(data)
}

/// `POST /ajax/session-data`. Port of `set_session_data`: merges the
/// posted object into whatever's already stored (not a full
/// overwrite), matching upstream's own `ud.update(new_data)`.
pub async fn set_session_data(State(state): State<AppState>, user: Option<Extension<AuthenticatedUser>>, Json(new_data): Json<Value>) -> Result<Json<Value>, ServerError> {
    let Some(new_obj) = new_data.as_object() else {
        return Err(ServerError::BadRequest("session data must be a JSON object".to_string()));
    };
    let username = effective_user(user);
    let Some(gate) = &state.auth else {
        return Ok(Json(serde_json::json!({})));
    };
    let mut merged = gate.users.get_session_data(&username);
    let merged_obj = merged.as_object_mut().expect("get_session_data always returns an object");
    for (k, v) in new_obj {
        merged_obj.insert(k.clone(), v.clone());
    }
    gate.users.set_session_data(&username, &merged).map_err(|e| ServerError::InternalServerError(e.to_string()))?;
    Ok(Json(merged))
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{header, Request, StatusCode};
    use tower::ServiceExt;

    use lexmechanic_db::cache::Cache;

    fn add_test_book(dir: &std::path::Path, cache: &Cache, title: &str, author: &str) -> i32 {
        let source = dir.join(format!("{title}.epub"));
        std::fs::write(&source, b"fake epub bytes").unwrap();
        let mut meta = lexmechanic_ebooks::metadata::MetaInformation::default();
        meta.title = title.to_string();
        meta.authors = vec![author.to_string()];
        cache.add_book(&source, &meta).unwrap()
    }

    fn test_app(book_count: usize) -> (tempfile::TempDir, axum::Router) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        for i in 0..book_count {
            add_test_book(dir.path(), &cache, &format!("Book {i}"), "Author");
        }
        let state = crate::AppState { libraries: None, cache: std::sync::Arc::new(cache), opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None, changes: crate::web_socket::new_change_broadcaster(), reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()), book_cache: std::sync::Arc::new(crate::books_cache::BookCache::open_temp()), jobs: std::sync::Arc::new(crate::jobs::JobsManager::new(4, std::time::Duration::from_secs(3600))), render_jobs: std::sync::Arc::new(crate::render_endpoints::RenderJobRegistry::new()), conversion_jobs: std::sync::Arc::new(crate::convert::ConversionJobRegistry::new()), news_jobs: std::sync::Arc::new(crate::news::NewsJobRegistry::new()), tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()), news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()), tts_voice: None, plugin_store: None, plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(lexmechanic_customize::registry::PluginRegistry::new())), };
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

    #[tokio::test]
    async fn book_returns_real_metadata_with_get_urls() {
        let (_dir, router) = test_app(1);
        let (status, body) = get_json(&router, "/ajax/book/1").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["title"], "Book 0");
        assert_eq!(body["authors"], serde_json::json!(["Author"]));
        assert_eq!(body["cover"], "/get/cover/1");
        assert_eq!(body["thumbnail"], "/get/thumb/1");
        assert_eq!(body["main_format"], serde_json::json!({"epub": "/get/epub/1"}));
        assert!(body.get("size").is_none(), "size should be stripped like upstream, got: {body}");
    }

    #[tokio::test]
    async fn book_404s_for_an_unknown_id() {
        let (_dir, router) = test_app(1);
        let (status, _) = get_json(&router, "/ajax/book/999").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn books_maps_ids_to_metadata_and_unknown_ids_to_null() {
        let (_dir, router) = test_app(2);
        let (status, body) = get_json(&router, "/ajax/books?ids=1,999").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["1"]["title"], "Book 0");
        assert_eq!(body["999"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn books_all_returns_every_book() {
        let (_dir, router) = test_app(3);
        let (status, body) = get_json(&router, "/ajax/books").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.as_object().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn categories_list_includes_authors_and_omits_empty_categories() {
        let (_dir, router) = test_app(1);
        let (status, body) = get_json(&router, "/ajax/categories").await;
        assert_eq!(status, StatusCode::OK);
        let names: Vec<&str> = body.as_array().unwrap().iter().map(|c| c["url"].as_str().unwrap()).collect();
        assert!(names.contains(&"/ajax/category/authors"));
        assert!(!names.contains(&"/ajax/category/series"), "no book has a series -- should be omitted, got: {body}");
    }

    #[tokio::test]
    async fn category_lists_items_and_paginates() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        add_test_book(dir.path(), &cache, "Book A", "Jane Doe");
        add_test_book(dir.path(), &cache, "Book B", "John Smith");
        let state = crate::AppState { libraries: None, cache: std::sync::Arc::new(cache), opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None, changes: crate::web_socket::new_change_broadcaster(), reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()), book_cache: std::sync::Arc::new(crate::books_cache::BookCache::open_temp()), jobs: std::sync::Arc::new(crate::jobs::JobsManager::new(4, std::time::Duration::from_secs(3600))), render_jobs: std::sync::Arc::new(crate::render_endpoints::RenderJobRegistry::new()), conversion_jobs: std::sync::Arc::new(crate::convert::ConversionJobRegistry::new()), news_jobs: std::sync::Arc::new(crate::news::NewsJobRegistry::new()), tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()), news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()), tts_voice: None, plugin_store: None, plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(lexmechanic_customize::registry::PluginRegistry::new())), };
        let router = crate::test_router(state);

        let (status, body) = get_json(&router, "/ajax/category/authors?num=1").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["total_num"], 2);
        assert_eq!(body["items"].as_array().unwrap().len(), 1);
        assert_eq!(body["items"][0]["count"], 1);
        assert!(body["items"][0]["url"].as_str().unwrap().starts_with("/ajax/books_in/authors/"));
    }

    #[tokio::test]
    async fn category_404s_for_an_unknown_category() {
        let (_dir, router) = test_app(1);
        let (status, _) = get_json(&router, "/ajax/category/bogus").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn books_in_lists_only_that_items_books() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        add_test_book(dir.path(), &cache, "Book A", "Jane Doe");
        add_test_book(dir.path(), &cache, "Book B", "John Smith");
        let state = crate::AppState { libraries: None, cache: std::sync::Arc::new(cache), opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None, changes: crate::web_socket::new_change_broadcaster(), reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()), book_cache: std::sync::Arc::new(crate::books_cache::BookCache::open_temp()), jobs: std::sync::Arc::new(crate::jobs::JobsManager::new(4, std::time::Duration::from_secs(3600))), render_jobs: std::sync::Arc::new(crate::render_endpoints::RenderJobRegistry::new()), conversion_jobs: std::sync::Arc::new(crate::convert::ConversionJobRegistry::new()), news_jobs: std::sync::Arc::new(crate::news::NewsJobRegistry::new()), tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()), news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()), tts_voice: None, plugin_store: None, plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(lexmechanic_customize::registry::PluginRegistry::new())), };
        let router = crate::test_router(state);

        let (_, cat) = get_json(&router, "/ajax/category/authors").await;
        let item_url = cat["items"][0]["url"].as_str().unwrap().to_string();

        let (status, body) = get_json(&router, &item_url).await;
        assert_eq!(status, StatusCode::OK);
        let ids = body["book_ids"].as_array().unwrap();
        assert_eq!(ids.len(), 1);
    }

    #[tokio::test]
    async fn books_in_returns_an_empty_list_for_an_unknown_item_id() {
        // Matches upstream's own `books_in`: an unknown item id within a
        // *valid* category is just an empty result, not a 404 -- only an
        // unknown *category* name 404s (see the next test).
        let (_dir, router) = test_app(1);
        let (status, body) = get_json(&router, "/ajax/books_in/authors/999999").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["book_ids"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn books_in_404s_for_an_unknown_category() {
        let (_dir, router) = test_app(1);
        let (status, _) = get_json(&router, "/ajax/books_in/bogus/1").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn search_returns_matching_book_ids() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        add_test_book(dir.path(), &cache, "Foundation", "Isaac Asimov");
        add_test_book(dir.path(), &cache, "Dune", "Frank Herbert");
        let state = crate::AppState { libraries: None, cache: std::sync::Arc::new(cache), opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None, changes: crate::web_socket::new_change_broadcaster(), reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()), book_cache: std::sync::Arc::new(crate::books_cache::BookCache::open_temp()), jobs: std::sync::Arc::new(crate::jobs::JobsManager::new(4, std::time::Duration::from_secs(3600))), render_jobs: std::sync::Arc::new(crate::render_endpoints::RenderJobRegistry::new()), conversion_jobs: std::sync::Arc::new(crate::convert::ConversionJobRegistry::new()), news_jobs: std::sync::Arc::new(crate::news::NewsJobRegistry::new()), tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()), news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()), tts_voice: None, plugin_store: None, plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(lexmechanic_customize::registry::PluginRegistry::new())), };
        let router = crate::test_router(state);

        let (status, body) = get_json(&router, "/ajax/search?query=Foundation").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["book_ids"], serde_json::json!([1]));
        assert_eq!(body["total_num"], 1);
    }

    #[tokio::test]
    async fn search_multi_field_sort_breaks_ties_on_the_second_field() {
        // Issue #759: a real comma-separated `sort` breaks a tie on
        // the first field using the second. Two books share the same
        // author (tying on `sort=authors`) but have different titles
        // -- sorting by "authors,title" must order them by title
        // within that tie, not leave them in whatever order the
        // first-field-only sort happened to produce.
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let zeta_id = add_test_book(dir.path(), &cache, "Zeta", "Same Author");
        let alpha_id = add_test_book(dir.path(), &cache, "Alpha", "Same Author");
        let state = crate::AppState { libraries: None, cache: std::sync::Arc::new(cache), opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None, changes: crate::web_socket::new_change_broadcaster(), reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()), book_cache: std::sync::Arc::new(crate::books_cache::BookCache::open_temp()), jobs: std::sync::Arc::new(crate::jobs::JobsManager::new(4, std::time::Duration::from_secs(3600))), render_jobs: std::sync::Arc::new(crate::render_endpoints::RenderJobRegistry::new()), conversion_jobs: std::sync::Arc::new(crate::convert::ConversionJobRegistry::new()), news_jobs: std::sync::Arc::new(crate::news::NewsJobRegistry::new()), tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()), news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()), tts_voice: None, plugin_store: None, plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(lexmechanic_customize::registry::PluginRegistry::new())), };
        let router = crate::test_router(state);

        let (status, body) = get_json(&router, "/ajax/search?sort=authors,title&sort_order=asc").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["sort"], "authors,title");
        assert_eq!(body["book_ids"], serde_json::json!([alpha_id, zeta_id]), "{body}");
    }

    /// The fast sort keys are exactly the keys the full rows give, for every
    /// field the fast path takes, on books with those fields set, unset and
    /// odd (multi-valued, fractional, missing).
    #[tokio::test]
    async fn fast_sort_keys_match_the_full_row_keys_for_every_fast_field() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let a = add_test_book(dir.path(), &cache, "The Zeta Book", "Zed Writer & Ann Other");
        let b = add_test_book(dir.path(), &cache, "alpha", "Bea Author");
        let _c = add_test_book(dir.path(), &cache, "Émile", "Unknown");
        for (field, value) in [("publisher", "Pub"), ("series", "Saga"), ("series_index", "2.5"), ("rating", "8"), ("tags", "b, a, c"), ("languages", "eng, fra"), ("pubdate", "2001-02-03T00:00:00+00:00"), ("comments", "x"), ("identifiers", "isbn:9780000000002")] {
            cache.set_field(a, field, value).unwrap();
        }
        cache.set_field(b, "series", "Another").unwrap();
        cache.set_field(b, "rating", "3").unwrap();
        let cache = std::sync::Arc::new(cache);
        let all: std::collections::HashSet<i32> = cache.all_book_ids().unwrap().into_iter().collect();
        let rows = super::fetch_rows_from_cache(cache.clone(), all.clone()).await.unwrap();
        for field in super::FAST_SORT_FIELDS {
            let mut fast = super::sort_keys(&cache, all.clone(), &[field]).await.unwrap();
            fast.sort_by_key(|(id, _)| *id);
            let files = super::FileSortData::load(&cache, &all, &[field.to_string()]).unwrap();
            let mut full: Vec<(i64, Vec<String>)> = rows.iter().map(|r| (r["id"].as_i64().unwrap(), vec![super::key_for(r, field, &files)])).collect();
            full.sort_by_key(|(id, _)| *id);
            assert_eq!(fast, full, "sort keys differ for {field}");
        }
        // A field the fast path doesn't know still sorts, through the full rows.
        let other = super::sort_keys(&cache, all.clone(), &["formats"]).await.unwrap();
        assert_eq!(other.len(), all.len());
    }

    fn app_for(cache: Cache) -> axum::Router {
        let state = crate::AppState { libraries: None, cache: std::sync::Arc::new(cache), opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None, changes: crate::web_socket::new_change_broadcaster(), reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()), book_cache: std::sync::Arc::new(crate::books_cache::BookCache::open_temp()), jobs: std::sync::Arc::new(crate::jobs::JobsManager::new(4, std::time::Duration::from_secs(3600))), render_jobs: std::sync::Arc::new(crate::render_endpoints::RenderJobRegistry::new()), conversion_jobs: std::sync::Arc::new(crate::convert::ConversionJobRegistry::new()), news_jobs: std::sync::Arc::new(crate::news::NewsJobRegistry::new()), tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()), news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()), tts_voice: None, plugin_store: None, plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(lexmechanic_customize::registry::PluginRegistry::new())), };
        crate::test_router(state)
    }

    /// A book whose one file is `bytes` long, so its size is known.
    fn add_sized_book(dir: &std::path::Path, cache: &Cache, title: &str, bytes: usize) -> i32 {
        let source = dir.join(format!("{title}.pdf"));
        std::fs::write(&source, vec![b'x'; bytes]).unwrap();
        let mut meta = lexmechanic_ebooks::metadata::MetaInformation::default();
        meta.title = title.to_string();
        meta.authors = vec!["Author".to_string()];
        cache.add_book(&source, &meta).unwrap()
    }

    #[tokio::test]
    async fn books_with_files_adds_size_folder_and_file_name_but_the_default_shape_is_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let id = add_sized_book(dir.path(), &cache, "Sized", 1234);
        let folder = cache.field_for(id, "path").unwrap().unwrap();
        let router = app_for(cache);

        let (_, plain) = get_json(&router, &format!("/ajax/books?ids={id}")).await;
        assert!(plain[id.to_string()].get("size").is_none(), "the calibre-compatible shape keeps size stripped: {plain}");
        assert!(plain[id.to_string()].get("folder").is_none());
        assert!(plain[id.to_string()].get("file_name").is_none());

        let (status, with) = get_json(&router, &format!("/ajax/books?ids={id}&files=1")).await;
        assert_eq!(status, StatusCode::OK);
        let row = &with[id.to_string()];
        assert_eq!(row["size"], 1234);
        assert_eq!(row["folder"], folder.as_str());
        assert_eq!(row["file_name"], "Sized.pdf");
    }

    #[tokio::test]
    async fn search_sorts_by_size_numerically_and_by_folder_and_file_name() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        // Sizes chosen so text order (10, 9, 100) differs from numeric (9, 10, 100).
        let nine = add_sized_book(dir.path(), &cache, "Nine", 9);
        let ten = add_sized_book(dir.path(), &cache, "Ten", 10);
        let hundred = add_sized_book(dir.path(), &cache, "Hundred", 100);
        let router = app_for(cache);

        let (_, asc) = get_json(&router, "/ajax/search?sort=size&sort_order=asc").await;
        assert_eq!(asc["book_ids"], serde_json::json!([nine, ten, hundred]), "{asc}");
        let (_, desc) = get_json(&router, "/ajax/search?sort=size&sort_order=desc").await;
        assert_eq!(desc["book_ids"], serde_json::json!([hundred, ten, nine]), "{desc}");

        // File names are Hundred.pdf < Nine.pdf < Ten.pdf; the folders follow the author/title layout.
        let (_, by_name) = get_json(&router, "/ajax/search?sort=file_name&sort_order=asc").await;
        assert_eq!(by_name["book_ids"], serde_json::json!([hundred, nine, ten]), "{by_name}");
        let (_, by_folder) = get_json(&router, "/ajax/search?sort=folder&sort_order=asc").await;
        let (_, folders) = get_json(&router, &format!("/ajax/books?ids={nine},{ten},{hundred}&files=1")).await;
        let order: Vec<String> = by_folder["book_ids"].as_array().unwrap().iter().map(|i| folders[i.to_string()]["folder"].as_str().unwrap().to_string()).collect();
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(order, sorted, "folder sort is ascending: {by_folder}");
    }

    #[test]
    fn numeric_keys_order_like_the_numbers() {
        let mut nums = [100.0, 9.0, 10.0, 0.0, 2.5, 1e9];
        let mut keys: Vec<(String, f64)> = nums.iter().map(|n| (super::numeric_key(*n), *n)).collect();
        keys.sort_by(|a, b| a.0.cmp(&b.0));
        nums.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(keys.iter().map(|(_, n)| *n).collect::<Vec<_>>(), nums);
    }

    #[tokio::test]
    async fn search_with_no_matches_returns_an_empty_list_not_404() {
        let (_dir, router) = test_app(1);
        let (status, body) = get_json(&router, "/ajax/search?query=nosuchbook").await;
        assert_eq!(status, StatusCode::OK, "ajax search should return an empty list, not 404, unlike opds::search");
        assert_eq!(body["book_ids"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn search_expands_a_saved_search_name_end_to_end() {
        // Real cross-crate wiring check for issue #422: a saved search
        // added via lexmechanic_db::cache::Cache is resolvable through the
        // real HTTP endpoint via a `search:name` query, with no
        // srv-side code needed for it (see this endpoint's own doc).
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        add_test_book(dir.path(), &cache, "Foundation", "Isaac Asimov");
        add_test_book(dir.path(), &cache, "Dune", "Frank Herbert");
        cache.saved_search_add("asimov books", "title:Foundation").unwrap();
        let state = crate::AppState { libraries: None, cache: std::sync::Arc::new(cache), opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None, changes: crate::web_socket::new_change_broadcaster(), reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()), book_cache: std::sync::Arc::new(crate::books_cache::BookCache::open_temp()), jobs: std::sync::Arc::new(crate::jobs::JobsManager::new(4, std::time::Duration::from_secs(3600))), render_jobs: std::sync::Arc::new(crate::render_endpoints::RenderJobRegistry::new()), conversion_jobs: std::sync::Arc::new(crate::convert::ConversionJobRegistry::new()), news_jobs: std::sync::Arc::new(crate::news::NewsJobRegistry::new()), tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()), news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()), tts_voice: None, plugin_store: None, plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(lexmechanic_customize::registry::PluginRegistry::new())), };
        let router = crate::test_router(state);

        let (status, body) = get_json(&router, "/ajax/search?query=search%3A%22asimov+books%22").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["book_ids"], serde_json::json!([1]));
    }

    #[tokio::test]
    async fn search_unparseable_query_404s() {
        let (_dir, router) = test_app(1);
        let (status, _) = get_json(&router, "/ajax/search?query=%28author%3Aasimov").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn library_info_reports_the_single_library() {
        let (_dir, router) = test_app(0);
        let (status, body) = get_json(&router, "/ajax/library-info").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["default_library"], "default");
        assert!(body["library_map"]["default"].is_string());
    }

    // Issue #258: the UI banner reads the tier, journal mode and in-use
    // signals from here.
    #[tokio::test]
    async fn library_info_reports_the_storage_tier_and_journal_mode() {
        let (_dir, router) = test_app(0);
        let (_, body) = get_json(&router, "/ajax/library-info").await;
        let storage = &body["storage"];
        assert!(storage["tier"].is_string());
        assert!(["wal", "delete"].contains(&storage["journal_mode"].as_str().unwrap()));
        assert!(storage["in_use"].is_array());
        assert!(storage["write_blocked_reason"].is_null());
    }

    // The name used to be the hardcoded string "Lexmechanic Library"
    // and the path was not reported at all, so nothing the UI could
    // display distinguished one open library from another.
    #[tokio::test]
    async fn library_info_names_the_real_library_and_its_path() {
        let dir = tempfile::tempdir().unwrap();
        let library = dir.path().join("Science Fiction");
        std::fs::create_dir(&library).unwrap();
        let cache = Cache::new(&library).unwrap();
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
            plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(lexmechanic_customize::registry::PluginRegistry::new())),
        };
        let router = crate::test_router(state);

        let (status, body) = get_json(&router, "/ajax/library-info").await;
        assert_eq!(status, StatusCode::OK);
        // The folder's own name, which is what a user calls a library.
        assert_eq!(body["library_map"]["default"], "Science Fiction");
        assert_eq!(body["library_path"].as_str().unwrap(), library.to_string_lossy());
    }

    #[tokio::test]
    async fn field_metadata_lists_real_fields_and_sortable_fields() {
        let (_dir, router) = test_app(1);
        let (status, body) = get_json(&router, "/ajax/field-metadata").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["field_metadata"]["title"]["datatype"], "text");
        assert_eq!(body["field_metadata"]["authors"]["is_category"], true);
        let sortable: Vec<String> = body["sortable_fields"].as_array().unwrap().iter().map(|pair| pair[0].as_str().unwrap().to_string()).collect();
        assert!(sortable.contains(&"title".to_string()));
        assert!(!sortable.contains(&"uuid".to_string()), "uuid is in the real exclusion set");
    }

    #[tokio::test]
    async fn virtual_libraries_lists_a_real_saved_map() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        cache.virtual_library_add("Unread", "not read:true").unwrap();
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
            conversion_jobs: std::sync::Arc::new(crate::convert::ConversionJobRegistry::new()), news_jobs: std::sync::Arc::new(crate::news::NewsJobRegistry::new()), tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()), news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()), tts_voice: None, plugin_store: None, plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(lexmechanic_customize::registry::PluginRegistry::new())),
        };
        let router = crate::test_router(state);
        let (status, body) = get_json(&router, "/ajax/virtual-libraries").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["Unread"], "not read:true");
    }

    #[tokio::test]
    async fn search_vl_restricts_to_the_intersection_of_query_and_virtual_library() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        add_test_book(dir.path(), &cache, "Foundation", "Isaac Asimov");
        add_test_book(dir.path(), &cache, "Dune", "Frank Herbert");
        cache.virtual_library_add("Asimov Only", "author:asimov").unwrap();
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
            conversion_jobs: std::sync::Arc::new(crate::convert::ConversionJobRegistry::new()), news_jobs: std::sync::Arc::new(crate::news::NewsJobRegistry::new()), tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()), news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()), tts_voice: None, plugin_store: None, plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(lexmechanic_customize::registry::PluginRegistry::new())),
        };
        let router = crate::test_router(state);

        // Every book matches "*", but vl=Asimov Only narrows to book 1.
        let (status, body) = get_json(&router, "/ajax/search?vl=Asimov+Only").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["book_ids"], serde_json::json!([1]));

        // vl + a query that also matches book 1 -> still book 1.
        let (_, body) = get_json(&router, "/ajax/search?query=title%3AFoundation&vl=Asimov+Only").await;
        assert_eq!(body["book_ids"], serde_json::json!([1]));

        // vl + a query that matches only the OTHER book -> empty.
        let (_, body) = get_json(&router, "/ajax/search?query=title%3ADune&vl=Asimov+Only").await;
        assert_eq!(body["book_ids"], serde_json::json!([]));
    }

    fn basic_auth_header(username: &str, password: &str) -> String {
        use base64::Engine;
        format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}")))
    }

    fn test_app_with_auth(username: &str, password: &str) -> (tempfile::TempDir, axum::Router) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let users = crate::users::UserManager::new(&dir.path().join("users.sqlite")).unwrap();
        users.add_user(username, password, false).unwrap();
        let auth = Some(std::sync::Arc::new(crate::auth::AuthGate::new(users, "calibre".to_string(), 0, 5)));
        let state = crate::AppState {
            libraries: None,
            cache: std::sync::Arc::new(cache),
            opts: std::sync::Arc::new(crate::opts::ServerOptions::default()),
            auth,
            changes: crate::web_socket::new_change_broadcaster(),
            reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()),
            book_cache: std::sync::Arc::new(crate::books_cache::BookCache::open_temp()),
            jobs: std::sync::Arc::new(crate::jobs::JobsManager::new(4, std::time::Duration::from_secs(3600))),
            render_jobs: std::sync::Arc::new(crate::render_endpoints::RenderJobRegistry::new()),
            conversion_jobs: std::sync::Arc::new(crate::convert::ConversionJobRegistry::new()), news_jobs: std::sync::Arc::new(crate::news::NewsJobRegistry::new()), tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()), news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()), tts_voice: None, plugin_store: None, plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(lexmechanic_customize::registry::PluginRegistry::new())),
        };
        (dir, crate::test_router(state))
    }

    #[tokio::test]
    async fn session_data_round_trips_for_a_real_authenticated_user() {
        let (_dir, router) = test_app_with_auth("alice", "hunter2");

        let req = Request::builder().uri("/ajax/session-data").header(header::AUTHORIZATION, basic_auth_header("alice", "hunter2")).body(Body::empty()).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&body).unwrap(), serde_json::json!({}));

        let req = Request::builder()
            .method("POST")
            .uri("/ajax/session-data")
            .header(header::AUTHORIZATION, basic_auth_header("alice", "hunter2"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"font_size": 16}"#))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let req = Request::builder().uri("/ajax/session-data").header(header::AUTHORIZATION, basic_auth_header("alice", "hunter2")).body(Body::empty()).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&body).unwrap(), serde_json::json!({"font_size": 16}));
    }

    #[tokio::test]
    async fn session_data_post_merges_rather_than_overwrites() {
        let (_dir, router) = test_app_with_auth("alice", "hunter2");
        let post = |body: &'static str| {
            Request::builder()
                .method("POST")
                .uri("/ajax/session-data")
                .header(header::AUTHORIZATION, basic_auth_header("alice", "hunter2"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap()
        };
        router.clone().oneshot(post(r#"{"font_size": 16}"#)).await.unwrap();
        router.clone().oneshot(post(r#"{"theme": "dark"}"#)).await.unwrap();

        let req = Request::builder().uri("/ajax/session-data").header(header::AUTHORIZATION, basic_auth_header("alice", "hunter2")).body(Body::empty()).unwrap();
        let resp = router.oneshot(req).await.unwrap();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&body).unwrap(), serde_json::json!({"font_size": 16, "theme": "dark"}));
    }

    #[tokio::test]
    async fn session_data_without_auth_configured_is_a_harmless_empty_no_op() {
        let (_dir, router) = test_app(0);
        let (status, body) = get_json(&router, "/ajax/session-data").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, serde_json::json!({}));
    }

    /// The library switcher in the UI reads `library_map`, so with one
    /// hardcoded entry there was nothing to switch to however many
    /// libraries the server actually had.
    #[tokio::test]
    async fn library_info_reports_every_library_the_server_hosts() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        drop(Cache::new(first.path()).unwrap());
        drop(Cache::new(second.path()).unwrap());
        let broker = std::sync::Arc::new(crate::library_broker::LibraryBroker::new(&[first.path().to_path_buf(), second.path().to_path_buf()]).unwrap());

        let default_cache = broker.get(None).unwrap();
        let state = crate::AppState { libraries: Some(broker.clone()), cache: default_cache, opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None, changes: crate::web_socket::new_change_broadcaster(), reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()), book_cache: std::sync::Arc::new(crate::books_cache::BookCache::open_temp()), jobs: std::sync::Arc::new(crate::jobs::JobsManager::new(4, std::time::Duration::from_secs(3600))), render_jobs: std::sync::Arc::new(crate::render_endpoints::RenderJobRegistry::new()), conversion_jobs: std::sync::Arc::new(crate::convert::ConversionJobRegistry::new()), news_jobs: std::sync::Arc::new(crate::news::NewsJobRegistry::new()), tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()), news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()), tts_voice: None, plugin_store: None, plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(lexmechanic_customize::registry::PluginRegistry::new())), };
        let router = crate::test_router(state);

        let (status, body) = get_json(&router, "/ajax/library-info").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let map = body["library_map"].as_object().expect("expected a library_map object");
        assert_eq!(map.len(), 2, "both libraries should be listed: {body}");
        assert_eq!(body["default_library"], broker.default_library_id(), "{body}");
    }

    /// Single-library mode keeps reporting the id it always has, so a
    /// client that remembers `default` is not broken by the above.
    #[tokio::test]
    async fn library_info_keeps_the_default_id_when_serving_one_library() {
        let (_dir, router) = test_app(0);
        let (status, body) = get_json(&router, "/ajax/library-info").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["default_library"], super::LIBRARY_ID, "{body}");
        assert_eq!(body["library_map"].as_object().unwrap().len(), 1, "{body}");
    }

    // --- sorting: typed comparison and the kept order ---

    /// The sort this module did before: every key through ICU collation.
    fn icu_only_order(mut keyed: Vec<(i64, Vec<String>)>, descending: bool) -> Vec<i64> {
        keyed.sort_by(|(_, a), (_, b)| {
            for (ka, kb) in a.iter().zip(b.iter()) {
                let cmp = if descending { lexmechanic_utils::icu::strcmp(kb, ka) } else { lexmechanic_utils::icu::strcmp(ka, kb) };
                if cmp != std::cmp::Ordering::Equal {
                    return cmp;
                }
            }
            std::cmp::Ordering::Equal
        });
        keyed.into_iter().map(|(id, _)| id).collect()
    }

    #[test]
    fn bytewise_comparison_is_used_only_for_keys_of_one_fixed_shape() {
        use super::bytewise_order_matches_icu as ok;
        assert!(ok(["2020-01-02T03:04:05+00:00", "", "1999-12-31T23:59:59+00:00"].into_iter()));
        assert!(ok(["0000000000000012.340000", "0000000000000002.500000"].into_iter()));
        assert!(ok(std::iter::empty()));
        assert!(!ok(["2020-01-02T03:04:05+00:00", "2020-01-02T03:04:05.123456+00:00"].into_iter()), "different fractional digits: '+' and '.' sort differently in ICU");
        assert!(!ok(["2020-01-02T03:04:05+00:00", "2020-01-02T03:04:05-05:00"].into_iter()), "a sign differs");
        assert!(!ok(["-000000000000001.000000", "0000000000000001.000000"].into_iter()));
        assert!(!ok(["apple", "Apple"].into_iter()), "text is collated");
    }

    #[test]
    fn typed_sort_orders_exactly_as_icu_collation_does() {
        // A small deterministic generator, so a failure repeats.
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut next = move |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        let words = ["apple", "Apple", "banana", "Éclair", "eclair", "zebra", "Zebra", "café", "cafe", "", "x-ray", "x ray"];
        for shape in 0..3 {
            let mut keyed: Vec<(i64, Vec<String>)> = Vec::new();
            for id in 1..=300i64 {
                let date = match shape {
                    // One fixed shape: sorted bytewise.
                    0 => format!("20{:02}-{:02}-{:02}T{:02}:{:02}:00+00:00", next(30), 1 + next(12), 1 + next(28), next(24), next(60)),
                    // Mixed fractional digits and zones: left to ICU.
                    1 => match next(3) {
                        0 => format!("20{:02}-01-01T00:00:00+00:00", next(30)),
                        1 => format!("20{:02}-01-01T00:00:00.{:06}+00:00", next(30), next(1_000_000)),
                        _ => String::new(),
                    },
                    _ => format!("{:020.6}", next(5) as f64 / 2.0),
                };
                let num = format!("0{:020.6}", next(40) as f64 / 4.0);
                keyed.push((id, vec![words[next(words.len() as u64) as usize].to_string(), date, num]));
            }
            for descending in [false, true] {
                for columns in [1usize, 2, 3] {
                    let want = icu_only_order(keyed.iter().map(|(id, k)| (*id, k[..columns].to_vec())).collect(), descending);
                    let mut got: Vec<(i64, Vec<String>)> = keyed.iter().map(|(id, k)| (*id, k[..columns].to_vec())).collect();
                    super::sort_keyed(&mut got, columns, descending);
                    assert_eq!(got.into_iter().map(|(id, _)| id).collect::<Vec<_>>(), want, "shape {shape}, {columns} column(s), descending {descending}");
                }
            }
        }
    }

    /// The order kept for the whole library, filtered by a search, is the
    /// order of sorting that search's books alone -- and an edit is seen.
    #[tokio::test]
    async fn the_kept_library_order_filtered_equals_sorting_the_result_directly() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let titles = ["The Zebra", "apple", "Apple", "Éclair", "banana", "A Cat", "Cat", "dog", "Dog Days", "emu", "Fox", "grape", "Hat", "ice", "Jam", "kite", "Lamp", "moon", "Nut", "owl"];
        let ids: Vec<i32> = titles.iter().enumerate().map(|(i, t)| add_test_book(dir.path(), &cache, t, if i % 3 == 0 { "Same Author" } else { "Other Person" })).collect();
        for (i, id) in ids.iter().enumerate() {
            cache.set_field(*id, "pubdate", &format!("20{:02}-0{}-1{}T00:00:00+00:00", 10 + (i * 7) % 13, 1 + i % 9, i % 10)).unwrap();
            cache.set_field(*id, "rating", &((i % 6) * 2).to_string()).unwrap();
        }
        let cache = std::sync::Arc::new(cache);
        for fields in [vec!["title"], vec!["authors", "title"], vec!["pubdate"], vec!["rating", "title"], vec!["size"], vec!["authors"], vec!["rating"], vec!["publisher"]] {
            for descending in [false, true] {
                let everything: std::collections::HashSet<i32> = ids.iter().copied().collect();
                let many: std::collections::HashSet<i32> = ids.iter().copied().filter(|i| i % 3 != 0).collect();
                let few: std::collections::HashSet<i32> = ids.iter().copied().take(4).collect();
                for subset in [&everything, &many, &few] {
                    let want = icu_only_order(super::sort_keys(&cache, subset.clone(), &fields).await.unwrap(), descending);
                    let got = super::sorted_ids(&cache, subset.clone(), &fields, descending).await.unwrap();
                    assert_eq!(got, want, "{fields:?} descending {descending} over {} books", subset.len());
                }
            }
        }
        // An edit moves the book, in the kept order too.
        let all: std::collections::HashSet<i32> = ids.iter().copied().collect();
        let before = super::sorted_ids(&cache, all.clone(), &["title"], false).await.unwrap();
        cache.set_field(ids[0], "title", "AAA first").unwrap();
        let after = super::sorted_ids(&cache, all.clone(), &["title"], false).await.unwrap();
        assert_ne!(before, after);
        assert_eq!(after[0], ids[0] as i64);
        assert_eq!(after, icu_only_order(super::sort_keys(&cache, all, &["title"]).await.unwrap(), false));
    }

    /// The kept order is keyed by what the sort reads: an edit to another
    /// field leaves it standing, an edit to a field it sorts on replaces it.
    /// Descending is taken from the ascending order, ties and all.
    #[tokio::test]
    async fn the_kept_order_survives_edits_to_fields_it_does_not_sort_on() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        // Several books with the very same title and author, so there are ties to keep in id order.
        let titles = ["Same", "Same", "Other", "Same", "Zed", "Other", "Aardvark", "Same"];
        let ids: Vec<i32> = titles.iter().enumerate().map(|(i, t)| add_test_book(dir.path(), &cache, t, if i % 2 == 0 { "Ann" } else { "Bob" })).collect();
        let cache = std::sync::Arc::new(cache);
        let all: std::collections::HashSet<i32> = ids.iter().copied().collect();
        let kept = |cache: &Cache, fields: &[&str]| {
            let store = cache.fields().unwrap();
            cache.cached_if_current::<super::SortedOrder>(&format!("sort:{}", fields.join(",")), store.fields_version(fields)).is_some()
        };
        for fields in [vec!["title"], vec!["authors"], vec!["title", "authors"]] {
            let asc = super::sorted_ids(&cache, all.clone(), &fields, false).await.unwrap();
            let desc = super::sorted_ids(&cache, all.clone(), &fields, true).await.unwrap();
            let want_desc = icu_only_order(super::sort_keys(&cache, all.clone(), &fields).await.unwrap(), true);
            assert_eq!(desc, want_desc, "{fields:?}: descending from the kept order keeps ties in id order");
            assert_ne!(desc, asc.iter().rev().copied().collect::<Vec<_>>(), "{fields:?}: not simply reversed, or the ties would run backwards");
            assert!(kept(&cache, &fields));
        }
        cache.set_field(ids[0], "rating", "4").unwrap();
        cache.set_field(ids[1], "comments", "<p>about</p>").unwrap();
        cache.set_field(ids[2], "tags", "red").unwrap();
        for fields in [vec!["title"], vec!["authors"], vec!["title", "authors"]] {
            assert!(kept(&cache, &fields), "{fields:?} is still kept after edits to rating, comments and tags");
        }
        assert!(!kept(&cache, &["rating"]), "nothing was kept for rating");
        cache.set_field(ids[0], "title", "Aaa").unwrap();
        assert!(!kept(&cache, &["title"]), "a title edit replaces the order of titles");
        assert!(kept(&cache, &["authors"]), "...and not the order of authors");
        let after = super::sorted_ids(&cache, all.clone(), &["title"], false).await.unwrap();
        assert_eq!(after, icu_only_order(super::sort_keys(&cache, all, &["title"]).await.unwrap(), false));
    }

    /// Only the requested category is built, and the list of categories is
    /// the same one the full computation gave.
    #[tokio::test]
    async fn category_endpoints_answer_from_one_category_and_agree_with_the_full_set() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let a = add_test_book(dir.path(), &cache, "One", "Ann Author");
        let b = add_test_book(dir.path(), &cache, "Two", "Ann Author");
        add_test_book(dir.path(), &cache, "Three", "Émile Zola");
        cache.set_field(a, "tags", "red, blue").unwrap();
        cache.set_field(b, "tags", "red").unwrap();
        let state = crate::AppState { libraries: None, cache: std::sync::Arc::new(cache), opts: std::sync::Arc::new(crate::opts::ServerOptions::default()), auth: None, changes: crate::web_socket::new_change_broadcaster(), reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()), book_cache: std::sync::Arc::new(crate::books_cache::BookCache::open_temp()), jobs: std::sync::Arc::new(crate::jobs::JobsManager::new(4, std::time::Duration::from_secs(3600))), render_jobs: std::sync::Arc::new(crate::render_endpoints::RenderJobRegistry::new()), conversion_jobs: std::sync::Arc::new(crate::convert::ConversionJobRegistry::new()), news_jobs: std::sync::Arc::new(crate::news::NewsJobRegistry::new()), tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()), news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()), tts_voice: None, plugin_store: None, plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(lexmechanic_customize::registry::PluginRegistry::new())), };
        let full = lexmechanic_db::categories::get_categories(&state.cache, "name", None).unwrap();
        let cache_handle = state.cache.clone();
        let router = crate::test_router(state);

        let (status, list) = get_json(&router, "/ajax/categories").await;
        assert_eq!(status, StatusCode::OK, "{list}");
        let urls: Vec<&str> = list.as_array().unwrap().iter().map(|c| c["url"].as_str().unwrap()).collect();
        let want: Vec<String> = crate::opds::CATEGORY_NAMES.iter().filter(|(k, _)| full.get(*k).is_some_and(|v| !v.is_empty())).map(|(k, _)| format!("/ajax/category/{k}")).collect();
        assert_eq!(urls, want.iter().map(String::as_str).collect::<Vec<_>>());
        assert!(!urls.contains(&"/ajax/category/series"), "an empty category is not listed");

        for (name, sort_order) in [("authors", "asc"), ("authors", "desc"), ("tags", "asc")] {
            let (status, body) = get_json(&router, &format!("/ajax/category/{name}?sort_order={sort_order}")).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            let mut expected: Vec<(String, usize)> = full[name].iter().map(|t| (t.name.clone(), t.count)).collect();
            if sort_order == "desc" {
                expected.reverse();
            }
            let got: Vec<(String, usize)> = body["items"].as_array().unwrap().iter().map(|i| (i["name"].as_str().unwrap().to_string(), i["count"].as_u64().unwrap() as usize)).collect();
            assert_eq!(got, expected, "{name} {sort_order}");
            assert_eq!(body["total_num"], expected.len());
        }
        // A write is seen by the next request, not an old kept answer.
        let (_, before) = get_json(&router, "/ajax/category/tags").await;
        assert_eq!(before["total_num"], 2);
        cache_handle.set_field(a, "tags", "red, blue, green").unwrap();
        let (_, after) = get_json(&router, "/ajax/category/tags").await;
        assert_eq!(after["total_num"], 3, "{after}");
    }
}
