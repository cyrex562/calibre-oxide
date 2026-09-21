//! Port of `old_src/src/calibre/db/annotations.py`'s server-side
//! merge algorithm, plus the annotation-storage slice of
//! `old_src/src/calibre/db/{backend,cache}.py` (issue #485, part of
//! #427's tracking epic) -- storage and merging for the generic
//! annotations system (bookmarks/highlights/last-read markers synced
//! from the in-browser reader), backing `calibre_srv::books`'s
//! `get_annotations`/`update_annotations` endpoints.
//!
//! # Scope: only what those two endpoints need
//!
//! [`annotations_map_for_book`], [`merge_annotations_for_book`] and
//! [`all_annotations`] are the upstream `Cache` read/write APIs
//! ported here. `all_annotations` was added for the library-wide
//! annotation browser (#816 item 1.8) -- until then this doc listed
//! it among the unported ones, which is worth noting because an
//! audit read that list as a list of things that *existed*.
//!
//! Still unported, each a separate follow-up once something needs it:
//! `search_annotations` (FTS), `all_annotations_for_book`,
//! `delete_annotations`/`update_annotations` (the by-id write API,
//! confusingly same-named as this module's own `merge_annotations`),
//! `restore_annotations`, and `save_annotations_list`.
//!
//! # Not ported: `cfi_sort_key`-based position sorting
//!
//! Upstream's `bookmark_sort_key`/`highlight_sort_key`/
//! `sort_annot_list_by_position_in_book`/`merge_annot_lists` all use
//! `calibre.ebooks.epub.cfi.parse.cfi_sort_key` (a real port already
//! exists at `calibre_ebooks::epub::cfi::parse::cfi_sort_key`) --
//! but tracing actual callers shows those four are only used by the
//! **desktop GUI viewer's own** annotation merge
//! (`gui2/viewer/annotations.py`), not by the server-side
//! `merge_annotations`/`merge_annotations_for_book` this module
//! ports. The server-side algorithm only ever compares timestamp
//! strings (`safe_timestamp_sort_key`) -- no CFI parsing needed here
//! at all. (An earlier draft of issue #485 assumed otherwise before
//! this was checked; corrected there too.)
//!
//! # Not ported: NFKC unicode normalization
//!
//! Upstream's `unicode_normalize` (NFKC-normalizes `searchable_text`
//! before storage) isn't ported -- `searchable_text` only feeds FTS
//! search over annotations (`search_annotations`, itself unported,
//! see above), so normalizing it has no observable effect yet. A
//! real, disclosed simplification, not a silent gap.

use std::collections::HashMap;

use anyhow::Result;
use serde_json::Value;

use crate::cache::Cache;

const MERGE_FIELDS: &[(&str, &str)] = &[("bookmark", "title"), ("highlight", "uuid")];

/// Port of `annot_db_data`: the `(annot_id, searchable_text)` derived
/// from an annotation's own JSON for storage, or `None` for any type
/// besides `bookmark`/`highlight` (matches upstream exactly --
/// `last-read` and any other type are computed during merge but
/// never actually persisted as an `annotations` table row, since
/// `save_annotations_for_book`'s own `if aid is None: continue` skips
/// them the same way).
fn annot_db_data(annot: &Value) -> Option<(String, String)> {
    let atype = annot.get("type")?.as_str()?.to_lowercase();
    match atype.as_str() {
        "bookmark" => {
            let title = annot.get("title")?.as_str()?.to_string();
            Some((title.clone(), title))
        }
        "highlight" => {
            let uuid = annot.get("uuid")?.as_str()?.to_string();
            let mut text = annot.get("highlighted_text").and_then(Value::as_str).unwrap_or("").to_string();
            let notes = annot.get("notes").and_then(Value::as_str).unwrap_or("");
            if !notes.is_empty() {
                text.push_str("\n\u{1f}\n");
                text.push_str(notes);
            }
            Some((uuid, text))
        }
        _ => None,
    }
}

/// Port of `safe_timestamp_sort_key`: the annotation's `timestamp`
/// string, or a value that sorts last if it's missing/not a string
/// (upstream's own fallback, `'zzzz'`).
fn safe_timestamp_sort_key(annot: &Value) -> &str {
    match annot.get("timestamp") {
        Some(Value::String(s)) => s.as_str(),
        _ => "zzzz",
    }
}

/// Port of `merge_annots_with_identical_field`: merges `a` and `b`
/// (an existing and an incoming annotation list of the same type),
/// keeping one annotation per distinct value of `field` (the most
/// recently timestamped one when both lists have a value for it).
fn merge_annots_with_identical_field(a: &[Value], b: &[Value], field: &str) -> Vec<Value> {
    let mut groups: HashMap<String, Vec<Value>> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for x in a.iter().chain(b.iter()) {
        let key = x.get(field).and_then(Value::as_str).unwrap_or("").to_string();
        let grp = groups.entry(key.clone()).or_insert_with(|| {
            order.push(key.clone());
            Vec::new()
        });
        grp.push(x.clone());
    }
    for grp in groups.values_mut() {
        grp.sort_by(|x, y| safe_timestamp_sort_key(y).cmp(safe_timestamp_sort_key(x)));
    }

    let mut seen = std::collections::HashSet::new();
    let mut ans = Vec::new();
    for x in a.iter().chain(b.iter()) {
        let key = x.get(field).and_then(Value::as_str).unwrap_or("").to_string();
        if seen.insert(key.clone()) {
            ans.push(groups[&key][0].clone());
        }
    }
    ans
}

/// Port of `merge_annotations`: merges a flat incoming annotation
/// list (`annots`, each a JSON object with a `"type"` field) into
/// `annots_map` (type -> list of existing annotations) in place.
/// `last-read` keeps only the single most recent entry (by
/// timestamp) across both sides; `bookmark`/`highlight` merge by
/// title/uuid via [`merge_annots_with_identical_field`]; every other
/// type is left untouched (upstream doesn't merge them either).
fn merge_annotations(annots: &[Value], annots_map: &mut HashMap<String, Vec<Value>>) {
    let mut incoming: HashMap<String, Vec<Value>> = HashMap::new();
    for annot in annots {
        if let Some(t) = annot.get("type").and_then(Value::as_str) {
            incoming.entry(t.to_string()).or_default().push(annot.clone());
        }
    }

    if let Some(lr) = incoming.get("last-read") {
        if !lr.is_empty() {
            let mut combined: Vec<Value> = annots_map.get("last-read").cloned().unwrap_or_default();
            combined.extend(lr.iter().cloned());
            if !combined.is_empty() {
                combined.sort_by(|x, y| safe_timestamp_sort_key(y).cmp(safe_timestamp_sort_key(x)));
                annots_map.insert("last-read".to_string(), vec![combined[0].clone()]);
            }
        }
    }

    for (annot_type, field) in MERGE_FIELDS {
        let Some(b) = incoming.get(*annot_type) else { continue };
        if b.is_empty() {
            continue;
        }
        let a = annots_map.get(*annot_type).cloned().unwrap_or_default();
        let merged = merge_annots_with_identical_field(&a, b, field);
        annots_map.insert((*annot_type).to_string(), merged);
    }
}

/// Port of `db.annotation_count_for_book` (issue #514's
/// `annotation_count()` template function): the total number of
/// stored annotations for `book_id`, across every format/user.
pub fn annotation_count_for_book(cache: &Cache, book_id: i32) -> Result<i64> {
    let conn = cache.backend.conn.lock().unwrap();
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM annotations WHERE book = ?1", [book_id], |row| row.get(0))?;
    Ok(count)
}

/// One row of [`all_annotations`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct AnnotationRow {
    pub id: i64,
    pub book_id: i32,
    pub format: String,
    pub user_type: String,
    pub user: String,
    /// The annotation's human-readable text: a bookmark's title, or a
    /// highlight's highlighted text. Upstream derives this the same
    /// way rather than storing it separately.
    pub text: String,
    pub annotation: Value,
}

/// Port of `backend.all_annotations`: every annotation in the
/// library, newest first.
///
/// `restrict_to_book_ids` is applied in Rust rather than in SQL,
/// matching upstream -- the id set comes from a search result and can
/// be larger than SQLite's parameter limit.
///
/// Rows whose `annot_data` will not parse, or which carry no `type`,
/// are skipped rather than failing the whole query: one corrupt row
/// should not make the annotation browser unusable.
pub fn all_annotations(
    cache: &Cache,
    restrict_to_user: Option<(&str, &str)>,
    annotation_type: Option<&str>,
    ignore_removed: bool,
    restrict_to_book_ids: Option<&std::collections::HashSet<i32>>,
    limit: Option<usize>,
) -> Result<Vec<AnnotationRow>> {
    let conn = cache.backend.conn.lock().unwrap();

    let mut sql = "SELECT id, book, format, user_type, user, annot_data FROM annotations".to_string();
    let mut clauses: Vec<&str> = Vec::new();
    let mut params: Vec<String> = Vec::new();
    if let Some((user_type, user)) = restrict_to_user {
        clauses.push(" user_type = ? AND user = ?");
        params.push(user_type.to_string());
        params.push(user.to_string());
    }
    if let Some(atype) = annotation_type {
        clauses.push(" annot_type = ? ");
        params.push(atype.to_string());
    }
    if !clauses.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&clauses.join(" AND "));
    }
    sql.push_str(" ORDER BY timestamp DESC ");

    let mut stmt = conn.prepare(&sql)?;
    let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p as &dyn rusqlite::ToSql).collect();
    let rows = stmt.query_map(param_refs.as_slice(), |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i32>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?, row.get::<_, String>(4)?, row.get::<_, String>(5)?))
    })?;

    let mut out = Vec::new();
    for row in rows {
        let (id, book_id, format, user_type, user, raw) = row?;
        if let Some(ids) = restrict_to_book_ids {
            if !ids.contains(&book_id) {
                continue;
            }
        }
        let Ok(annot) = serde_json::from_str::<Value>(&raw) else { continue };
        let Some(atype) = annot.get("type").and_then(Value::as_str) else { continue };
        if ignore_removed && annot.get("removed").and_then(Value::as_bool).unwrap_or(false) {
            continue;
        }
        let text = match atype {
            "bookmark" => annot.get("title").and_then(Value::as_str).unwrap_or("").to_string(),
            "highlight" => annot.get("highlighted_text").and_then(Value::as_str).unwrap_or("").to_string(),
            _ => String::new(),
        };
        out.push(AnnotationRow { id, book_id, format, user_type, user, text, annotation: annot });
        if let Some(limit) = limit {
            if out.len() >= limit {
                break;
            }
        }
    }
    Ok(out)
}

/// Port of `db.annotations_map_for_book`: every stored annotation for
/// `book_id`/`fmt`/`user_type`/`user`, grouped by its own `"type"`.
pub fn annotations_map_for_book(cache: &Cache, book_id: i32, fmt: &str, user_type: &str, user: &str) -> Result<HashMap<String, Vec<Value>>> {
    let fmt = fmt.to_uppercase();
    let conn = cache.backend.conn.lock().unwrap();
    let mut stmt = conn.prepare("SELECT annot_data FROM annotations WHERE book = ?1 AND format = ?2 AND user_type = ?3 AND user = ?4")?;
    let rows = stmt.query_map((book_id, &fmt, user_type, user), |row| row.get::<_, String>(0))?;
    let mut ans: HashMap<String, Vec<Value>> = HashMap::new();
    for row in rows {
        let raw = row?;
        if let Ok(annot) = serde_json::from_str::<Value>(&raw) {
            if let Some(t) = annot.get("type").and_then(Value::as_str) {
                ans.entry(t.to_string()).or_default().push(annot);
            }
        }
    }
    Ok(ans)
}

/// Port of `db.set_annotations_for_book` (via `save_annotations_for_book`):
/// fully replaces the stored annotations for this
/// `book_id`/`fmt`/`user_type`/`user`. Only `bookmark`/`highlight`
/// entries actually get a row -- see [`annot_db_data`]'s own doc.
fn set_annotations_for_book(cache: &Cache, book_id: i32, fmt: &str, annots_list: &[(Value, f64)], user_type: &str, user: &str) -> Result<()> {
    let fmt = fmt.to_uppercase();
    let conn = cache.backend.conn.lock().unwrap();
    conn.execute("INSERT OR IGNORE INTO annotations_dirtied (book) VALUES (?1)", [book_id])?;
    conn.execute("DELETE FROM annotations WHERE book = ?1 AND format = ?2 AND user_type = ?3 AND user = ?4", (book_id, &fmt, user_type, user))?;
    for (annot, ts) in annots_list {
        let Some((aid, text)) = annot_db_data(annot) else { continue };
        let Some(atype) = annot.get("type").and_then(Value::as_str) else { continue };
        let data = serde_json::to_string(annot)?;
        conn.execute(
            "INSERT OR REPLACE INTO annotations (book, format, user_type, user, timestamp, annot_id, annot_type, annot_data, searchable_text) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            (book_id, &fmt, user_type, user, ts, aid, atype.to_lowercase(), data, text),
        )?;
    }
    Ok(())
}

/// Best-effort ISO8601 -> Unix-epoch-seconds, matching upstream's own
/// `(parse_iso8601(ts) - EPOCH).total_seconds()`. An unparseable
/// timestamp maps to `0.0` rather than erroring the whole merge --
/// matches this crate's general "narrow, disclosed, don't fail the
/// whole request over one bad field" posture elsewhere.
fn iso8601_to_epoch_secs(ts: &str) -> f64 {
    chrono::DateTime::parse_from_rfc3339(ts).map(|dt| dt.timestamp() as f64 + dt.timestamp_subsec_nanos() as f64 / 1e9).unwrap_or(0.0)
}

/// Port of `db.merge_annotations_for_book`: loads the existing
/// annotations for this book/fmt/user, merges `annots_list` (a flat
/// list of incoming annotation JSON objects) into them via
/// [`merge_annotations`], and persists the result.
pub fn merge_annotations_for_book(cache: &Cache, book_id: i32, fmt: &str, annots_list: &[Value], user_type: &str, user: &str) -> Result<()> {
    let mut amap = annotations_map_for_book(cache, book_id, fmt, user_type, user)?;
    merge_annotations(annots_list, &mut amap);
    let mut alist = Vec::new();
    for annots in amap.into_values() {
        for annot in annots {
            let ts = annot.get("timestamp").and_then(Value::as_str).map(iso8601_to_epoch_secs).unwrap_or(0.0);
            alist.push((annot, ts));
        }
    }
    set_annotations_for_book(cache, book_id, fmt, &alist, user_type, user)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `annotations.book` has a real foreign-key trigger against
    /// `books.id` -- every test needs an actual book row to attach
    /// annotations to.
    fn seed_book(cache: &Cache) -> i32 {
        let conn = cache.backend.conn.lock().unwrap();
        conn.execute("INSERT INTO books (title) VALUES ('Test Book')", []).unwrap();
        conn.last_insert_rowid() as i32
    }

    #[test]
    fn annotations_map_for_book_is_empty_for_a_book_with_no_annotations() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
       let book_id = seed_book(&cache);
        let map = annotations_map_for_book(&cache, book_id, "epub", "web", "alice").unwrap();
        assert!(map.is_empty());
    }

    fn bookmark(title: &str, timestamp: &str) -> Value {
        serde_json::json!({"type": "bookmark", "title": title, "timestamp": timestamp, "pos": "epubcfi(/6/2)", "pos_type": "epubcfi"})
    }

    fn highlight(uuid: &str, timestamp: &str, text: &str) -> Value {
        serde_json::json!({"type": "highlight", "uuid": uuid, "timestamp": timestamp, "start_cfi": "epubcfi(/6/2!/4/2)", "end_cfi": "epubcfi(/6/2!/4/4)", "highlighted_text": text})
    }

    fn last_read(timestamp: &str) -> Value {
        serde_json::json!({"type": "last-read", "timestamp": timestamp, "cfi": "epubcfi(/6/2)"})
    }

    #[test]
    fn merging_a_new_bookmark_adds_it() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let book_id = seed_book(&cache);
        merge_annotations_for_book(&cache, book_id, "epub", &[bookmark("Chapter 1", "2026-01-01T00:00:00+00:00")], "web", "alice").unwrap();

        let map = annotations_map_for_book(&cache, book_id, "epub", "web", "alice").unwrap();
        assert_eq!(map["bookmark"].len(), 1);
        assert_eq!(map["bookmark"][0]["title"], "Chapter 1");
    }

    #[test]
    fn merging_a_bookmark_with_the_same_title_keeps_only_the_newer_one() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let book_id = seed_book(&cache);
        merge_annotations_for_book(&cache, book_id, "epub", &[bookmark("Chapter 1", "2026-01-01T00:00:00+00:00")], "web", "alice").unwrap();
        merge_annotations_for_book(&cache, book_id, "epub", &[bookmark("Chapter 1", "2026-06-01T00:00:00+00:00")], "web", "alice").unwrap();

        let map = annotations_map_for_book(&cache, book_id, "epub", "web", "alice").unwrap();
        assert_eq!(map["bookmark"].len(), 1, "same title should replace, not duplicate");
        assert_eq!(map["bookmark"][0]["timestamp"], "2026-06-01T00:00:00+00:00");
    }

    #[test]
    fn merging_a_bookmark_with_an_older_timestamp_keeps_the_existing_one() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let book_id = seed_book(&cache);
        merge_annotations_for_book(&cache, book_id, "epub", &[bookmark("Chapter 1", "2026-06-01T00:00:00+00:00")], "web", "alice").unwrap();
        merge_annotations_for_book(&cache, book_id, "epub", &[bookmark("Chapter 1", "2026-01-01T00:00:00+00:00")], "web", "alice").unwrap();

        let map = annotations_map_for_book(&cache, book_id, "epub", "web", "alice").unwrap();
        assert_eq!(map["bookmark"][0]["timestamp"], "2026-06-01T00:00:00+00:00");
    }

    #[test]
    fn different_bookmarks_and_highlights_all_survive_a_merge() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let book_id = seed_book(&cache);
        merge_annotations_for_book(&cache, book_id, "epub", &[bookmark("Chapter 1", "2026-01-01T00:00:00+00:00"), highlight("uuid-1", "2026-01-02T00:00:00+00:00", "hello")], "web", "alice").unwrap();
        merge_annotations_for_book(&cache, book_id, "epub", &[bookmark("Chapter 2", "2026-01-03T00:00:00+00:00"), highlight("uuid-2", "2026-01-04T00:00:00+00:00", "world")], "web", "alice").unwrap();

        let map = annotations_map_for_book(&cache, book_id, "epub", "web", "alice").unwrap();
        assert_eq!(map["bookmark"].len(), 2);
        assert_eq!(map["highlight"].len(), 2);
    }

    #[test]
    fn last_read_keeps_only_the_single_most_recent_entry_within_one_merge_call() {
        // last-read is never actually persisted as a row (see
        // `last_read_annotations_are_never_persisted_as_a_real_row`
        // below), so this only matters when a single request submits
        // several last-read entries at once (e.g. syncing more than
        // one device's position in one batch) -- it does not survive
        // across separate `merge_annotations_for_book` calls.
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let book_id = seed_book(&cache);
        let mut amap = HashMap::new();
        merge_annotations(&[last_read("2026-01-01T00:00:00+00:00"), last_read("2026-06-01T00:00:00+00:00"), last_read("2026-03-01T00:00:00+00:00")], &mut amap);
        assert_eq!(amap["last-read"].len(), 1);
        assert_eq!(amap["last-read"][0]["timestamp"], "2026-06-01T00:00:00+00:00");

        // Still real end-to-end behavior worth confirming: the merge
        // does complete without error even though last-read has no
        // persisted row (see `merge_annotations_for_book`'s own
        // `annot_db_data` gate).
        merge_annotations_for_book(&cache, book_id, "epub", &[last_read("2026-01-01T00:00:00+00:00")], "web", "alice").unwrap();
    }

    #[test]
    fn last_read_annotations_are_never_persisted_as_a_real_row() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let book_id = seed_book(&cache);
        merge_annotations_for_book(&cache, book_id, "epub", &[last_read("2026-01-01T00:00:00+00:00")], "web", "alice").unwrap();

        // Matches upstream: last-read participates in the in-memory
        // merge but has no `annot_db_data`, so `save_annotations_for_book`
        // silently drops it rather than storing a row -- a second
        // read of the same book/user starts from nothing again.
        let conn = cache.backend.conn.lock().unwrap();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM annotations WHERE book = ?1", [book_id], |r| r.get(0)).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn annotations_for_different_users_are_isolated() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let book_id = seed_book(&cache);
        merge_annotations_for_book(&cache, book_id, "epub", &[bookmark("Alice's mark", "2026-01-01T00:00:00+00:00")], "web", "alice").unwrap();
        merge_annotations_for_book(&cache, book_id, "epub", &[bookmark("Bob's mark", "2026-01-01T00:00:00+00:00")], "web", "bob").unwrap();

        let alice_map = annotations_map_for_book(&cache, book_id, "epub", "web", "alice").unwrap();
        let bob_map = annotations_map_for_book(&cache, book_id, "epub", "web", "bob").unwrap();
        assert_eq!(alice_map["bookmark"].len(), 1);
        assert_eq!(alice_map["bookmark"][0]["title"], "Alice's mark");
        assert_eq!(bob_map["bookmark"].len(), 1);
        assert_eq!(bob_map["bookmark"][0]["title"], "Bob's mark");
    }

    #[test]
    fn annotations_for_different_formats_are_isolated() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let book_id = seed_book(&cache);
        merge_annotations_for_book(&cache, book_id, "epub", &[bookmark("EPUB mark", "2026-01-01T00:00:00+00:00")], "web", "alice").unwrap();
        merge_annotations_for_book(&cache, book_id, "azw3", &[bookmark("AZW3 mark", "2026-01-01T00:00:00+00:00")], "web", "alice").unwrap();

        let epub_map = annotations_map_for_book(&cache, book_id, "epub", "web", "alice").unwrap();
        let azw3_map = annotations_map_for_book(&cache, book_id, "azw3", "web", "alice").unwrap();
        assert_eq!(epub_map["bookmark"][0]["title"], "EPUB mark");
        assert_eq!(azw3_map["bookmark"][0]["title"], "AZW3 mark");
    }
}

#[cfg(test)]
mod all_annotations_tests {
    use super::*;

    /// Builds a library with two books carrying real annotations, via
    /// the module's own write path rather than hand-written SQL, so
    /// the read path is tested against what the writer really stores.
    fn library_with_annotations() -> (tempfile::TempDir, Cache) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        for (title, author) in [("One", "Ann"), ("Two", "Bo")] {
            let path = dir.path().join(format!("{title}.epub"));
            std::fs::write(&path, b"x").unwrap();
            let mut meta = calibre_ebooks::metadata::MetaInformation::default();
            meta.title = title.to_string();
            meta.authors = vec![author.to_string()];
            cache.add_book(&path, &meta).unwrap();
        }

        let highlight = serde_json::json!({"type": "highlight", "uuid": "h1", "timestamp": "2026-01-01T00:00:00Z", "highlighted_text": "a memorable line"});
        let bookmark = serde_json::json!({"type": "bookmark", "title": "Chapter 3", "timestamp": "2026-01-02T00:00:00Z"});
        merge_annotations_for_book(&cache, 1, "epub", &[highlight], "local", "viewer").unwrap();
        merge_annotations_for_book(&cache, 2, "epub", &[bookmark], "local", "viewer").unwrap();
        (dir, cache)
    }

    #[test]
    fn returns_every_annotation_in_the_library() {
        let (_d, cache) = library_with_annotations();

        let all = all_annotations(&cache, None, None, false, None, None).unwrap();

        assert_eq!(all.len(), 2, "both books' annotations should come back: {all:?}");
        assert!(all.iter().any(|a| a.book_id == 1 && a.text == "a memorable line"));
        assert!(all.iter().any(|a| a.book_id == 2 && a.text == "Chapter 3"));
    }

    /// Upstream derives the display text per type rather than storing
    /// it: a bookmark shows its title, a highlight its highlighted
    /// text. Getting this wrong gives a browser full of blank rows.
    #[test]
    fn derives_display_text_from_the_annotation_type() {
        let (_d, cache) = library_with_annotations();

        let all = all_annotations(&cache, None, None, false, None, None).unwrap();
        let highlight = all.iter().find(|a| a.book_id == 1).unwrap();
        let bookmark = all.iter().find(|a| a.book_id == 2).unwrap();

        assert_eq!(highlight.text, "a memorable line");
        assert_eq!(bookmark.text, "Chapter 3");
    }

    #[test]
    fn filters_by_annotation_type() {
        let (_d, cache) = library_with_annotations();

        let highlights = all_annotations(&cache, None, Some("highlight"), false, None, None).unwrap();

        assert_eq!(highlights.len(), 1);
        assert_eq!(highlights[0].book_id, 1);
    }

    #[test]
    fn restricts_to_a_set_of_books() {
        let (_d, cache) = library_with_annotations();

        let ids: std::collections::HashSet<i32> = std::iter::once(2).collect();
        let only_two = all_annotations(&cache, None, None, false, Some(&ids), None).unwrap();

        assert_eq!(only_two.len(), 1);
        assert_eq!(only_two[0].book_id, 2);
    }

    #[test]
    fn honours_a_limit() {
        let (_d, cache) = library_with_annotations();
        assert_eq!(all_annotations(&cache, None, None, false, None, Some(1)).unwrap().len(), 1);
    }

    #[test]
    fn filters_by_user() {
        let (_d, cache) = library_with_annotations();

        assert_eq!(all_annotations(&cache, Some(("local", "viewer")), None, false, None, None).unwrap().len(), 2);
        assert_eq!(all_annotations(&cache, Some(("local", "nobody")), None, false, None, None).unwrap().len(), 0);
    }

    /// One unparseable row must not take down the whole browser.
    #[test]
    fn skips_a_corrupt_row_instead_of_failing() {
        let (_d, cache) = library_with_annotations();
        {
            let conn = cache.backend.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO annotations (book, format, user_type, user, timestamp, annot_id, annot_type, annot_data, searchable_text) VALUES (1, 'EPUB', 'local', 'viewer', 99, 'bad', 'highlight', 'not json', '')",
                [],
            )
            .unwrap();
        }

        let all = all_annotations(&cache, None, None, false, None, None).unwrap();

        assert_eq!(all.len(), 2, "the two good rows should still come back");
    }
}
