//! Comparing duplicates side by side, and the user's own groups (tranche 16).
//!
//! * `GET /duplicates/compare/{lib}?ids=1,2,3` -- 2 to 8 books (400 outside
//!   that), **any** books, detected as duplicates or not. Always read from the
//!   library now (not from the review snapshot, which may be old). Answers
//!   `{books, pairs, suggested_keeper, keeper_reason, pdf}`:
//!   * `books`: per book what a review card shows -- built by the review
//!     snapshot's own per-book builder ([`duplicates_review::book_json_from`]):
//!     title, authors, files (format, path, size, mtime), page count, text
//!     layer, `pdf.image_only`, cover URL, `format_list` -- and `suggested`.
//!   * `pairs`: one per pair `{a, b, identical_bytes, identical_formats,
//!     exempt, pdf, evidence}`. `identical_bytes` is `true` when a format of
//!     each has the same content hash, `false` when every file is hashed and
//!     none match, and `null` when a file has no hash yet. `pdf` is `null` when
//!     either book has no PDF, `{"fingerprinted": false, "missing": [..]}`
//!     until both are fingerprinted (not an error), and otherwise the
//!     comparison from the cached fingerprints: `kind` (a `pdf_*` reason, or
//!     `null` when the library scan would not call them a match), the graded
//!     `confidence`, `detail`, `visual_match`, the sample pages compared
//!     (`pages`: `{a, b, distance, matched}`, 0-based) with `compared`,
//!     `matched` and `mean_distance`, the text similarity `text_jaccard`, and
//!     the page counts. `evidence` is the title/author/identifier evidence the
//!     matcher finds between the two, `[]` if none.
//!   * `suggested_keeper`/`keeper_reason`: [`lexmechanic_db::resolve::choose_keeper`]
//!     over the set.
//! * `GET /duplicates/manual-groups/{lib}` -- the user's groups
//!   ([`lexmechanic_db::dedupe_manual_groups`]) with their live members.
//! * `POST /duplicates/manual-groups/{lib}` -- `{book_ids}` creates a group
//!   (at least two books; books already in a group join it), `{group_id, add:
//!   [..]}` adds, `{group_id, remove: [..]}` removes (the group goes when
//!   fewer than two remain). Answers `{manual_group, group, removed}`: the
//!   stored group (`null` if gone) and the review snapshot's group it is part
//!   of (`null` with no scan), which has any detected group it overlaps folded
//!   in. Putting books in a group removes the exemptions that group overrules.
//! * `POST /duplicates/manual-groups/remove/{lib}` -- `{group_id}`.
//! * `POST /duplicates/refresh-books/{lib}` -- `{book_ids}`: the caller has
//!   changed these books itself (title, file name, folder) and so the snapshot
//!   is not stale for it: re-reads them into the snapshot and moves its
//!   watermark, so a resolve is not refused for the caller's own edits.

use std::collections::{HashMap, HashSet};

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use lexmechanic_db::cache::Cache;
use lexmechanic_db::dedupe::{self, BookFacts, PdfBookInfo, Strength};
use lexmechanic_db::dedupe_exemptions;
use lexmechanic_db::dedupe_manual_groups as manual_groups;
use lexmechanic_db::pdf_similarity::{self, LibraryFingerprint, StopSlots};
use lexmechanic_db::resolve;

use crate::duplicates_review::{self, blocking, book_json_from, cache_for, group_json, internal, manual_sets, round2, snapshot_for, update_snapshot};
use crate::errors::ServerError;
use crate::AppState;

/// Books one comparison takes.
pub const MIN_COMPARE: usize = 2;
pub const MAX_COMPARE: usize = 8;

#[derive(Debug, Deserialize)]
pub struct CompareQuery {
    pub ids: Option<String>,
}

fn parse_ids(raw: Option<&str>) -> Result<Vec<i32>, ServerError> {
    let mut ids: Vec<i32> = Vec::new();
    for part in raw.unwrap_or("").split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let id: i32 = part.parse().map_err(|_| ServerError::BadRequest(format!("{part:?} is not a book id")))?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    if ids.len() < MIN_COMPARE || ids.len() > MAX_COMPARE {
        return Err(ServerError::BadRequest(format!("compare takes {MIN_COMPARE} to {MAX_COMPARE} different books (got {})", ids.len())));
    }
    Ok(ids)
}

fn reason_of(kind: pdf_similarity::PdfKind) -> &'static str {
    match kind {
        pdf_similarity::PdfKind::Resave => Strength::PdfResave.reason(),
        pdf_similarity::PdfKind::OcrVariant => Strength::PdfOcr.reason(),
        pdf_similarity::PdfKind::TextMatch => Strength::PdfText.reason(),
    }
}

fn has_pdf(facts: &BookFacts) -> bool {
    facts.files.iter().any(|f| f.format.eq_ignore_ascii_case("pdf"))
}

/// The PDF half of a pair.
fn pdf_pair_json(a: &lexmechanic_db::resolve::KeeperCandidate, b: &lexmechanic_db::resolve::KeeperCandidate, by_book: &HashMap<i32, &LibraryFingerprint>, stops: &StopSlots) -> Value {
    if !has_pdf(&a.facts) || !has_pdf(&b.facts) {
        return Value::Null;
    }
    let (Some(fa), Some(fb)) = (by_book.get(&a.id), by_book.get(&b.id)) else {
        let missing: Vec<i32> = [a.id, b.id].into_iter().filter(|id| !by_book.contains_key(id)).collect();
        return json!({"fingerprinted": false, "missing": missing});
    };
    let c = pdf_similarity::compare(&fa.fingerprint, &fb.fingerprint, stops);
    let (compared, matched, mean, pages) = match &c.alignment {
        Some(m) => (m.compared, m.matched, m.mean_distance, m.pages.iter().map(|p| json!({"a": p.a, "b": p.b, "distance": p.distance, "matched": p.matched})).collect::<Vec<_>>()),
        None => (0, 0, 0.0, Vec::new()),
    };
    json!({
        "fingerprinted": true,
        "kind": c.found.as_ref().map(|f| reason_of(f.kind)),
        "confidence": c.found.as_ref().map(|f| round2(f.confidence)),
        "detail": c.found.as_ref().map(|f| f.detail.clone()),
        "visual_match": c.visual_match,
        "aligned": c.alignment.is_some(),
        "compared": compared,
        "matched": matched,
        "mean_distance": (mean as f64 * 10.0).round() / 10.0,
        "pages": pages,
        "text_jaccard": c.text_jaccard.map(round2),
        "page_counts": [fa.fingerprint.page_count, fb.fingerprint.page_count],
        "has_text": [fa.fingerprint.has_text, fb.fingerprint.has_text],
    })
}

/// Which formats of `a` and `b` have the same content, and whether that is
/// known: `(identical formats, every file hashed)`.
fn identical_formats(a: &BookFacts, b: &BookFacts, hashes: &HashMap<(i32, String), (String, i64)>) -> (Vec<String>, bool) {
    let hash_of = |id: i32, format: &str| hashes.iter().find(|((book, f), _)| *book == id && f.eq_ignore_ascii_case(format)).map(|(_, v)| v);
    let mut all_hashed = true;
    for (book, files) in [(a.id, &a.files), (b.id, &b.files)] {
        for f in files {
            if f.size > 0 && hash_of(book, &f.format).is_none() {
                all_hashed = false;
            }
        }
    }
    let mut same: Vec<String> = Vec::new();
    for fa in &a.files {
        let Some((ha, sa)) = hash_of(a.id, &fa.format) else { continue };
        if *sa > 0 && b.files.iter().any(|fb| hash_of(b.id, &fb.format).is_some_and(|(hb, sb)| *sb > 0 && hb == ha)) && !same.contains(&fa.format) {
            same.push(fa.format.clone());
        }
    }
    (same, all_hashed)
}

fn compare_core(cache: &Cache, library_id: &str, ids: &[i32]) -> Result<Value, ServerError> {
    let records = dedupe::load_records(cache).map_err(internal)?;
    let books = duplicates_review::load_books_from(cache, &records, ids).map_err(internal)?;
    for id in ids {
        if !books.contains_key(id) {
            return Err(ServerError::NotFound(format!("no book with id {id}")));
        }
    }
    let library = cache.backend.library_path.clone();

    let loaded = cache.checksums().pdf_fingerprints().map_err(|e| internal(e.into()))?;
    let infos: HashMap<i32, PdfBookInfo> = dedupe::pdf_book_infos(&loaded);
    let facts: HashMap<i32, BookFacts> = books.iter().map(|(id, b)| (*id, b.facts.clone())).collect();
    let pdf_infos: HashMap<i32, PdfBookInfo> = ids.iter().filter_map(|id| infos.get(id).map(|p| (*id, *p))).collect();
    let candidates = resolve::candidates(ids, &facts, &pdf_infos);
    // A set of PDFs is ranked as PDFs (text layer first) even if some are not fingerprinted yet; they rank last.
    let keeper = resolve::choose_keeper(candidates.iter().all(|c| has_pdf(&c.facts)), &candidates);

    let by_book: HashMap<i32, &LibraryFingerprint> = loaded.iter().flat_map(|l| l.books.iter().filter(|b| ids.contains(b)).map(move |b| (*b, l))).collect();
    let stops = StopSlots::from_refs(loaded.iter().map(|l| &l.fingerprint));

    // Content hashes of just these books' files.
    let hashes: HashMap<(i32, String), (String, i64)> = cache.checksums().all_format_records().map_err(|e| internal(e.into()))?.into_iter().filter(|((book, _), _)| ids.contains(book)).map(|(k, (hash, size, _))| (k, (hash, size))).collect();

    let subset: Vec<dedupe::BookRecord> = ids.iter().filter_map(|id| records.iter().find(|r| r.id == *id).cloned()).collect();
    let cannot = dedupe_exemptions::cannot_link(cache, &subset).map_err(internal)?;

    let book_values: Vec<Value> = ids
        .iter()
        .map(|id| {
            let mut v = book_json_from(&books, *id, &library, library_id, true);
            v["suggested"] = json!(keeper.as_ref().is_some_and(|k| k.id == *id));
            v
        })
        .collect();

    let mut pairs = Vec::new();
    for (i, a) in candidates.iter().enumerate() {
        for b in &candidates[i + 1..] {
            let (same, all_hashed) = identical_formats(&a.facts, &b.facts, &hashes);
            let identical: Value = if !same.is_empty() { json!(true) } else if all_hashed { json!(false) } else { Value::Null };
            let pair_records: Vec<dedupe::BookRecord> = subset.iter().filter(|r| r.id == a.id || r.id == b.id).cloned().collect();
            let evidence: Vec<Value> = dedupe::find_groups_full(&pair_records, &[], &[], &[], Strength::Fuzzy, &dedupe_exemptions::CannotLink::new())
                .into_iter()
                .flat_map(|g| g.evidence)
                .map(|e| json!({"reason": e.strength.reason(), "confidence": round2(e.confidence), "detail": e.detail}))
                .collect();
            pairs.push(json!({
                "a": a.id,
                "b": b.id,
                "identical_bytes": identical,
                "identical_formats": same,
                "exempt": cannot.get(&a.id).is_some_and(|s| s.contains(&b.id)),
                "pdf": pdf_pair_json(a, b, &by_book, &stops),
                "evidence": evidence,
            }));
        }
    }
    let pdf_books = candidates.iter().filter(|c| has_pdf(&c.facts)).count();
    Ok(json!({
        "ids": ids,
        "books": book_values,
        "pairs": pairs,
        "suggested_keeper": keeper.as_ref().map(|k| k.id),
        "keeper_reason": keeper.as_ref().map(|k| k.reason.clone()),
        "pdf": {"books": pdf_books, "fingerprinted": candidates.iter().filter(|c| has_pdf(&c.facts) && by_book.contains_key(&c.id)).count()},
    }))
}

/// `GET /duplicates/compare/{library_id}?ids=1,2,3`.
pub async fn compare(State(state): State<AppState>, Path(library_id): Path<String>, Query(query): Query<CompareQuery>) -> Result<Json<Value>, ServerError> {
    let ids = parse_ids(query.ids.as_deref())?;
    let cache = cache_for(&state, &library_id)?;
    Ok(Json(blocking(move || compare_core(&cache, &library_id, &ids)).await?))
}

// ---------------------------------------------------------------------------
// The user's groups
// ---------------------------------------------------------------------------

fn manual_group_json(cache: &Cache, g: &manual_groups::ManualGroup) -> Value {
    let members: Vec<Value> = g
        .members
        .iter()
        .map(|m| {
            let book_id = cache.book_id_for_uuid(&m.uuid).ok().flatten();
            json!({"uuid": m.uuid, "title": m.title, "authors": m.authors, "book_id": book_id, "present": book_id.is_some()})
        })
        .collect();
    json!({"id": g.id, "created_at": g.created_at, "members": members, "book_ids": members.iter().filter_map(|m| m["book_id"].as_i64()).collect::<Vec<_>>()})
}

/// `GET /duplicates/manual-groups/{library_id}`.
pub async fn manual_groups_list(State(state): State<AppState>, Path(library_id): Path<String>) -> Result<Json<Value>, ServerError> {
    let cache = cache_for(&state, &library_id)?;
    let out = blocking(move || {
        manual_groups::prune(&cache).map_err(internal)?;
        let list = manual_groups::list(&cache).map_err(internal)?;
        let snap = snapshot_for(&cache);
        let groups: Vec<Value> = list
            .iter()
            .map(|g| {
                let mut v = manual_group_json(&cache, g);
                v["snapshot_group_id"] = json!(snap.as_ref().and_then(|s| s.groups.iter().find(|sg| sg.manual_id.as_deref() == Some(g.id.as_str())).map(|sg| sg.id.clone())));
                v
            })
            .collect();
        Ok(json!({"count": groups.len(), "groups": groups}))
    })
    .await?;
    Ok(Json(out))
}

#[derive(Debug, Default, Deserialize)]
pub struct ManualBody {
    #[serde(default)]
    pub book_ids: Vec<i32>,
    pub group_id: Option<String>,
    #[serde(default)]
    pub add: Vec<i32>,
    #[serde(default)]
    pub remove: Vec<i32>,
}

fn require_books(cache: &Cache, ids: &[i32]) -> Result<(), ServerError> {
    for id in ids {
        if cache.book_uuid(*id).map_err(internal)?.is_none() {
            return Err(ServerError::NotFound(format!("no book with id {id}")));
        }
    }
    Ok(())
}

/// Brings the snapshot (if there is one) up to date with the stored groups,
/// loading the books it has not seen, and answers with the snapshot's group
/// that holds the user's group `id`.
fn sync_snapshot(cache: &Cache, library_id: &str, group_id: Option<&str>) -> Result<Value, ServerError> {
    let manual = manual_sets(cache).map_err(internal)?;
    let Some(snap) = snapshot_for(cache) else { return Ok(Value::Null) };
    let missing: Vec<i32> = manual.iter().flat_map(|m| m.book_ids.iter().copied()).filter(|id| !snap.books.contains_key(id)).collect();
    let loaded = if missing.is_empty() { HashMap::new() } else { duplicates_review::load_books(cache, &missing).map_err(internal)? };
    update_snapshot(cache, Some(&snap.watermark), true, |s| {
        s.books.extend(loaded);
        s.manual = manual;
    });
    let (Some(id), Some(now)) = (group_id, snapshot_for(cache)) else { return Ok(Value::Null) };
    let library = cache.backend.library_path.clone();
    Ok(now.groups.iter().find(|g| g.manual_id.as_deref() == Some(id)).map(|g| group_json(&now, g, &library, library_id, true, true)).unwrap_or(Value::Null))
}

/// `POST /duplicates/manual-groups/{library_id}`.
pub async fn manual_groups_post(State(state): State<AppState>, Path(library_id): Path<String>, Json(body): Json<ManualBody>) -> Result<Json<Value>, ServerError> {
    let cache = cache_for(&state, &library_id)?;
    let out = blocking(move || {
        let (group, removed) = match body.group_id.as_deref() {
            None => {
                let mut ids = body.book_ids.clone();
                ids.sort_unstable();
                ids.dedup();
                if ids.len() < 2 {
                    return Err(ServerError::BadRequest("a group needs at least two different books".to_string()));
                }
                require_books(&cache, &ids)?;
                (Some(manual_groups::create(&cache, &ids).map_err(internal)?), false)
            }
            Some(gid) => {
                if manual_groups::get(&cache, gid).map_err(internal)?.is_none() {
                    return Err(ServerError::NotFound(format!("no group {gid}")));
                }
                if !body.add.is_empty() {
                    require_books(&cache, &body.add)?;
                    (Some(manual_groups::add(&cache, gid, &body.add).map_err(internal)?), false)
                } else if !body.remove.is_empty() {
                    let left = manual_groups::remove_books(&cache, gid, &body.remove).map_err(internal)?;
                    let gone = left.is_none();
                    (left, gone)
                } else {
                    return Err(ServerError::BadRequest("send {add: [..]} or {remove: [..]} with a group_id".to_string()));
                }
            }
        };
        let snapshot_group = sync_snapshot(&cache, &library_id, group.as_ref().map(|g| g.id.as_str()))?;
        Ok(json!({"manual_group": group.as_ref().map(|g| manual_group_json(&cache, g)), "group": snapshot_group, "removed": removed}))
    })
    .await?;
    Ok(Json(out))
}

#[derive(Debug, Deserialize)]
pub struct RemoveGroupBody {
    pub group_id: String,
}

/// `POST /duplicates/manual-groups/remove/{library_id}`.
pub async fn manual_groups_remove(State(state): State<AppState>, Path(library_id): Path<String>, Json(body): Json<RemoveGroupBody>) -> Result<Json<Value>, ServerError> {
    let cache = cache_for(&state, &library_id)?;
    let out = blocking(move || {
        if !manual_groups::delete(&cache, &body.group_id).map_err(internal)? {
            return Err(ServerError::NotFound(format!("no group {}", body.group_id)));
        }
        sync_snapshot(&cache, &library_id, None)?;
        Ok(json!({"removed": true, "group_id": body.group_id}))
    })
    .await?;
    Ok(Json(out))
}

#[derive(Debug, Deserialize)]
pub struct RefreshBody {
    #[serde(default)]
    pub book_ids: Vec<i32>,
}

/// `POST /duplicates/refresh-books/{library_id}`.
pub async fn refresh_books(State(state): State<AppState>, Path(library_id): Path<String>, Json(body): Json<RefreshBody>) -> Result<Json<Value>, ServerError> {
    let cache = cache_for(&state, &library_id)?;
    let out = blocking(move || {
        let Some(snap) = snapshot_for(&cache) else { return Ok(json!({"refreshed": 0, "has_scan": false})) };
        let wanted: HashSet<i32> = body.book_ids.iter().copied().collect();
        let ids: Vec<i32> = snap.books.keys().copied().filter(|id| wanted.contains(id)).collect();
        let fresh = duplicates_review::load_books(&cache, &ids).map_err(internal)?;
        let n = fresh.len();
        // The library's own changes since the scan are the caller's: the
        // watermark follows, as for the other routes that change it.
        update_snapshot(&cache, Some(&snap.watermark), true, |s| {
            for (id, book) in fresh {
                // A book whose file is gone from its folder keeps its old row.
                s.books.insert(id, book);
            }
        });
        let stale = snapshot_for(&cache).map(|s| s.watermark != cache.change_watermark());
        Ok(json!({"refreshed": n, "has_scan": true, "stale": stale}))
    })
    .await?;
    Ok(Json(out))
}
