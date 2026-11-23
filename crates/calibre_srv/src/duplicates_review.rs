//! Reviewing and resolving duplicate groups at scale (tranche 12).
//!
//! `POST /duplicates/scan` (see [`crate::duplicates`]) finds the groups. On a
//! 10,000-book library that is hundreds of groups, so this module keeps the
//! result **in memory** (one [`Snapshot`] per library) and serves it in pages,
//! and adds the actions that work through it.
//!
//! # The snapshot
//!
//! Built by every scan (route or job). It holds the groups with a stable id
//! (a hash of the sorted member uuids, so the same books keep the same id
//! across rescans), the suggested keeper and why, and everything the panel
//! shows about each book (files with size and relative path, pages / text
//! layer, record richness), loaded in a handful of batched queries. It is
//! tagged with the library's [`Cache::change_watermark`]; when the library
//! changes afterwards `GET /duplicates/groups` still serves it but says
//! `"stale": true`, and resolving refuses (409) until the library is
//! rescanned. Routes in this module that change the library themselves
//! (exempt, resolve, drop) update the snapshot and keep it current, so the
//! user keeps their place; changes made elsewhere are caught by the watermark.
//!
//! # Your groups
//!
//! Groups the user made ([`calibre_db::dedupe_manual_groups`]) are part of the
//! snapshot as groups with reason `"manual"`, confidence 1. The snapshot keeps
//! the *detected* groups and the *manual* sets apart and **derives** the groups
//! from both ([`Snapshot::derive`]): a manual set and the detected groups that
//! share a book with it are one group, which keeps every piece of the detected
//! evidence and adds a "grouped by you" note per pair. Every change to either
//! side re-derives, so adding a book to a manual group, removing one, or
//! resolving a group needs no rescan. Resolve treats `manual` like the other
//! metadata-only tiers: never in the default tiers, only on an explicit tick.
//!
//! # Routes
//!
//! * `GET /duplicates/groups/{lib}?offset&limit&reason=a,b&min_confidence&max_confidence&path&sort&order`
//!   -- a page of groups plus facets. `sort` is `confidence` (default),
//!   `size` (books in the group), `reason` (strongest tier first) or
//!   `reclaimable` (bytes); `order` is `asc` or `desc` (each sort's natural
//!   direction is descending). `limit` defaults to 50, at most 200.
//!   `path` is a case-insensitive prefix of a file or folder path, relative to
//!   the library. `facets.reason` counts groups per tier under every filter
//!   *except* `reason`, so the tier chips show what choosing them would give.
//! * `POST /duplicates/drop/{lib}` -- `{group_ids, adopt?}`: forget groups
//!   (after the UI merged or deleted them with the existing routes).
//!   `adopt` (default true) says the library changes since the scan are the
//!   caller's own, so the snapshot is not stale.
//! * `POST /duplicates/exempt/{lib}` -- `{group_id}` or `{book_ids}`: "not
//!   duplicates" for every pair among them ([`calibre_db::dedupe_exemptions`]).
//!   Manual groups holding two or more of those books lose them (the newest
//!   action wins).
//! * `GET /duplicates/compare/{lib}?ids=1,2,3` and the `manual-groups` routes
//!   are in [`crate::duplicates_compare`].
//! * `GET /duplicates/exemptions/{lib}`, `POST /duplicates/exemptions/remove/{lib}`
//!   (`{id}` or `{all: true}`) -- the Dismissed list and undo.
//! * `POST /duplicates/resolve/{lib}` -- the bulk resolve job, below.
//! * `GET /duplicates/runs/{lib}`, `POST /duplicates/runs/{lib}/{run_id}/restore`.
//!
//! # Resolve
//!
//! Body `{dry_run (default true), action, tiers, min_confidence, path_prefix,
//! group_ids, keepers, keep_duplicate_formats, plan_fingerprint}`.
//!
//! * **Selection.** Explicit `group_ids` select exactly those groups.
//!   Otherwise groups are selected by `tiers` (reasons; **default
//!   `["content"]`**, so title/author, identifier and fuzzy groups are never
//!   included unless named), at least `min_confidence`, and under
//!   `path_prefix`.
//! * **Keeper.** The suggested keeper (and its reason) unless `keepers`
//!   (`{group_id: book_id}`) overrides it.
//! * **Action.** `keep_best_trash_rest` trashes the others into the library
//!   trash; `merge_as_formats` merges them into the keeper (merge engine,
//!   `merge_and_delete`) which then trashes them.
//! * **Dry run is mandatory.** It answers `{plan_fingerprint, groups, totals,
//!   skipped, ...}` and changes nothing. Executing needs `dry_run: false` and
//!   the fingerprint; it is refused with 409 if the plan differs from what
//!   the user saw (the library changed, groups were dropped, ...). It starts
//!   a cancellable `resolve` library job and answers 202 with `run_id`.
//! * **Undo.** Every book the run trashes is tagged with the `run_id` in its
//!   trash entry; `POST /duplicates/runs/{lib}/{run_id}/restore` restores them
//!   all. Trashing is fully undoable until the trash expires. A merge is
//!   **not** fully undone by that: the sources come back but the keeper keeps
//!   the formats and metadata folded into it.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path as FsPath;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use calibre_db::cache::Cache;
use calibre_db::dedupe::{self, BookFacts, Evidence, PdfBookInfo, Strength};
use calibre_db::dedupe_exemptions;
use calibre_db::resolve::{self, Keeper, PlannedResolve, ResolveAction};
use calibre_db::scan_pipeline::JobControl;

use crate::errors::ServerError;
use crate::library_jobs::{self, JobKind, JobParams, Started};
use crate::AppState;

/// The most groups one page returns.
pub const PAGE_MAX: usize = 200;
const PAGE_DEFAULT: usize = 50;

// ---------------------------------------------------------------------------
// The snapshot
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SnapBook {
    pub id: i32,
    pub uuid: String,
    pub title: String,
    pub authors: Vec<String>,
    pub facts: BookFacts,
    pub pdf: Option<PdfBookInfo>,
}

#[derive(Debug, Clone)]
pub struct SnapGroup {
    /// Hash of the sorted member uuids.
    pub id: String,
    pub strength: Strength,
    pub confidence: f32,
    pub books: Vec<i32>,
    pub evidence: Vec<Evidence>,
    pub keeper: Keeper,
    /// What trashing everything but the keeper would free.
    pub bytes_reclaimable: i64,
    /// Lower-cased relative paths of every file and folder, for the path filter.
    pub paths_lc: Vec<String>,
    /// The user's group this is (or contains), see [`calibre_db::dedupe_manual_groups`].
    pub manual_id: Option<String>,
}

/// A user-made group, resolved to the books that exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualSet {
    pub id: String,
    pub book_ids: Vec<i32>,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub generation: u64,
    /// [`Cache::change_watermark`] when the scan began (or when the last
    /// local change was folded in).
    pub watermark: String,
    pub min: Strength,
    pub created_at: u64,
    /// What the matcher found, as it found it.
    pub detected: Vec<SnapGroup>,
    /// The user's own groups.
    pub manual: Vec<ManualSet>,
    /// What the panel shows: derived from `detected` and `manual`.
    pub groups: Vec<SnapGroup>,
    pub books: HashMap<i32, SnapBook>,
    pub pdf_status: calibre_db::pdf_similarity::PdfStatus,
    pub empty_files: Vec<Value>,
    pub exemptions_applied: usize,
}

static GENERATION: AtomicU64 = AtomicU64::new(1);

fn registry() -> &'static Mutex<HashMap<String, Arc<Snapshot>>> {
    static REG: OnceLock<Mutex<HashMap<String, Arc<Snapshot>>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

fn key_of(cache: &Cache) -> String {
    cache.backend.library_path.to_string_lossy().to_string()
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The library's current snapshot, if a scan has produced one.
pub fn snapshot_for(cache: &Cache) -> Option<Arc<Snapshot>> {
    registry().lock().unwrap().get(&key_of(cache)).cloned()
}

fn store(cache: &Cache, snapshot: Snapshot) -> Arc<Snapshot> {
    let snapshot = Arc::new(snapshot);
    registry().lock().unwrap().insert(key_of(cache), snapshot.clone());
    snapshot
}

/// Forgets a library's snapshot (tests; a library being closed).
pub fn forget(cache: &Cache) {
    registry().lock().unwrap().remove(&key_of(cache));
}

/// The snapshot's view of a group of books: its keeper and what resolving it
/// would free. `None` when there is no book to keep.
#[allow(clippy::too_many_arguments)]
fn make_group(books: &HashMap<i32, SnapBook>, mut ids: Vec<i32>, strength: Strength, confidence: f32, evidence: Vec<Evidence>, pdf_group: bool, manual_id: Option<String>) -> Option<SnapGroup> {
    ids.sort_unstable();
    ids.dedup();
    let uuids: Vec<String> = ids.iter().filter_map(|id| books.get(id).map(|b| b.uuid.clone())).collect();
    let facts: HashMap<i32, BookFacts> = ids.iter().filter_map(|id| books.get(id).map(|b| (*id, b.facts.clone()))).collect();
    let pdf: HashMap<i32, PdfBookInfo> = ids.iter().filter_map(|id| books.get(id).and_then(|b| b.pdf.map(|p| (*id, p)))).collect();
    let candidates = resolve::candidates(&ids, &facts, &pdf);
    let keeper = resolve::choose_keeper(pdf_group, &candidates)?;
    let others: Vec<&BookFacts> = ids.iter().filter(|id| **id != keeper.id).filter_map(|id| facts.get(id)).collect();
    let keeper_facts = facts.get(&keeper.id).cloned().unwrap_or_default();
    let bytes_reclaimable = resolve::bytes_freed(ResolveAction::KeepBestTrashRest, &keeper_facts, &others);
    let mut paths_lc: Vec<String> = Vec::new();
    for id in &ids {
        if let Some(f) = facts.get(id) {
            if !f.folder.is_empty() {
                paths_lc.push(f.folder.to_lowercase());
            }
            paths_lc.extend(f.files.iter().map(|file| file.path.to_lowercase()));
        }
    }
    Some(SnapGroup { id: dedupe_exemptions::set_id(&uuids), strength, confidence, books: ids, evidence, keeper, bytes_reclaimable, paths_lc, manual_id })
}

/// The user's groups as the snapshot holds them.
pub fn manual_sets(cache: &Cache) -> anyhow::Result<Vec<ManualSet>> {
    Ok(calibre_db::dedupe_manual_groups::live(cache)?.into_iter().map(|g| ManualSet { id: g.id, book_ids: g.book_ids }).collect())
}

/// What the panel shows about each of `ids`, read from the library now (the
/// snapshot's own copy can be old, and a book the user just grouped may not be
/// in it at all).
pub fn load_books(cache: &Cache, ids: &[i32]) -> anyhow::Result<HashMap<i32, SnapBook>> {
    load_books_from(cache, &dedupe::load_records(cache)?, ids)
}

/// [`load_books`] given the library's records already read.
pub fn load_books_from(cache: &Cache, records: &[dedupe::BookRecord], ids: &[i32]) -> anyhow::Result<HashMap<i32, SnapBook>> {
    let by_id: HashMap<i32, &dedupe::BookRecord> = records.iter().map(|r| (r.id, r)).collect();
    let mut ids: Vec<i32> = ids.iter().copied().filter(|id| by_id.contains_key(id)).collect();
    ids.sort_unstable();
    ids.dedup();
    let facts = dedupe::load_facts(cache, &ids, &by_id)?;
    let pdf = dedupe::pdf_book_infos(&cache.checksums().pdf_fingerprints()?);
    let mut out = HashMap::with_capacity(ids.len());
    for id in ids {
        let r = by_id[&id];
        out.insert(id, SnapBook { id, uuid: r.uuid.clone(), title: r.title.clone(), authors: r.authors.clone(), facts: facts.get(&id).cloned().unwrap_or(BookFacts { id, ..Default::default() }), pdf: pdf.get(&id).copied() });
    }
    Ok(out)
}

impl Snapshot {
    /// Recomputes `groups` from `detected` and `manual`: books joined by either
    /// are one group. A detected group no manual set touches is passed through
    /// as it is; a group with a manual set in it has reason `manual`,
    /// confidence 1, every detected piece of evidence, and a "grouped by you"
    /// note for each pair of the user's set.
    pub fn derive(&mut self) {
        let n = self.detected.len();
        let node_books: Vec<&[i32]> = self.detected.iter().map(|g| g.books.as_slice()).chain(self.manual.iter().map(|m| m.book_ids.as_slice())).collect();
        let mut parent: Vec<usize> = (0..node_books.len()).collect();
        fn find(parent: &mut [usize], mut x: usize) -> usize {
            while parent[x] != x {
                parent[x] = parent[parent[x]];
                x = parent[x];
            }
            x
        }
        let mut owner: HashMap<i32, usize> = HashMap::new();
        for (i, books) in node_books.iter().enumerate() {
            for b in *books {
                match owner.get(b) {
                    Some(&o) => {
                        let (ra, rb) = (find(&mut parent, o), find(&mut parent, i));
                        if ra != rb {
                            parent[ra.max(rb)] = ra.min(rb);
                        }
                    }
                    None => {
                        owner.insert(*b, i);
                    }
                }
            }
        }
        let mut components: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for i in 0..node_books.len() {
            let root = find(&mut parent, i);
            components.entry(root).or_default().push(i);
        }

        let mut groups: Vec<SnapGroup> = Vec::with_capacity(components.len());
        for nodes in components.values() {
            if let [only] = nodes.as_slice() {
                if *only < n {
                    groups.push(self.detected[*only].clone());
                    continue;
                }
            }
            let (detected, manual): (Vec<usize>, Vec<usize>) = nodes.iter().copied().partition(|i| *i < n);
            let mut ids: Vec<i32> = nodes.iter().flat_map(|i| node_books[*i].iter().copied()).collect();
            ids.sort_unstable();
            ids.dedup();
            let mut evidence: Vec<Evidence> = Vec::new();
            for m in &manual {
                let set = &self.manual[*m - n].book_ids;
                if let Some((first, rest)) = set.split_first() {
                    evidence.extend(rest.iter().map(|other| Evidence { a: *first, b: *other, strength: Strength::Manual, confidence: 1.0, detail: "grouped by you".to_string() }));
                }
            }
            let pdf_group = detected.iter().any(|i| self.detected[*i].strength.is_pdf());
            for d in &detected {
                evidence.extend(self.detected[*d].evidence.iter().cloned());
            }
            evidence.sort_by(|x, y| y.strength.cmp(&x.strength).then(y.confidence.total_cmp(&x.confidence)).then((x.a, x.b).cmp(&(y.a, y.b))));
            evidence.truncate(dedupe::MAX_EVIDENCE);
            let manual_id = manual.first().map(|m| self.manual[*m - n].id.clone());
            if let Some(g) = make_group(&self.books, ids, Strength::Manual, 1.0, evidence, pdf_group, manual_id) {
                groups.push(g);
            }
        }
        groups.sort_by(|a, b| b.strength.cmp(&a.strength).then(a.books.first().cmp(&b.books.first())));
        self.groups = groups;
    }
}

/// Scans the library and stores the result as its snapshot.
pub fn build_snapshot(cache: &Cache, min: Strength, control: Option<&JobControl>) -> anyhow::Result<Arc<Snapshot>> {
    if let Some(c) = control {
        c.set_phase("matching");
    }
    // Read before scanning: a write during the scan then leaves the snapshot stale.
    let watermark = cache.change_watermark();
    let outcome = dedupe::scan_library_full(cache, min)?;
    if let Some(c) = control {
        c.set_phase("describing");
        c.total.store(outcome.groups.len() as u64, Ordering::Relaxed);
    }
    let _ = calibre_db::dedupe_manual_groups::prune(cache);
    let manual = manual_sets(cache)?;
    let records: HashMap<i32, &dedupe::BookRecord> = outcome.records.iter().map(|r| (r.id, r)).collect();
    let mut ids: Vec<i32> = outcome.groups.iter().flat_map(|g| g.book_ids.iter().copied()).chain(manual.iter().flat_map(|m| m.book_ids.iter().copied())).collect();
    ids.sort_unstable();
    ids.dedup();
    let facts = dedupe::load_facts(cache, &ids, &records)?;

    let mut books: HashMap<i32, SnapBook> = HashMap::with_capacity(ids.len());
    for &id in &ids {
        let Some(r) = records.get(&id) else { continue };
        books.insert(id, SnapBook { id, uuid: r.uuid.clone(), title: r.title.clone(), authors: r.authors.clone(), facts: facts.get(&id).cloned().unwrap_or(BookFacts { id, ..Default::default() }), pdf: outcome.pdf.get(&id).copied() });
    }

    let mut detected = Vec::with_capacity(outcome.groups.len());
    for g in &outcome.groups {
        if control.is_some_and(|c| c.is_cancelled()) {
            break;
        }
        if let Some(group) = make_group(&books, g.book_ids.clone(), g.strength, g.confidence, g.evidence.clone(), g.strength.is_pdf(), None) {
            detected.push(group);
        }
        if let Some(c) = control {
            c.seen.fetch_add(1, Ordering::Relaxed);
        }
    }

    let empty_files: Vec<Value> = outcome
        .empty_files
        .iter()
        .map(|(id, format)| {
            let r = records.get(id);
            json!({"book_id": id, "format": format, "title": r.map(|r| r.title.as_str()).unwrap_or(""), "authors": r.map(|r| r.authors.clone()).unwrap_or_default()})
        })
        .collect();

    let mut snapshot = Snapshot { generation: GENERATION.fetch_add(1, Ordering::SeqCst), watermark, min, created_at: now_secs(), detected, manual, groups: Vec::new(), books, pdf_status: outcome.status, empty_files, exemptions_applied: outcome.exemptions_applied };
    snapshot.derive();
    Ok(store(cache, snapshot))
}

/// Drops `group_ids` from the snapshot, and (when `adopt` and the snapshot was
/// current at `expected_watermark`) moves its watermark to the library's now.
pub(crate) fn update_snapshot(cache: &Cache, expected_watermark: Option<&str>, adopt: bool, edit: impl FnOnce(&mut Snapshot)) {
    let mut reg = registry().lock().unwrap();
    let Some(current) = reg.get(&key_of(cache)).cloned() else { return };
    let mut next = (*current).clone();
    let was_current = expected_watermark.map(|w| w == current.watermark).unwrap_or(true);
    edit(&mut next);
    next.derive();
    if adopt && was_current {
        next.watermark = cache.change_watermark();
    }
    reg.insert(key_of(cache), Arc::new(next));
}

/// Forgets the groups with these ids (as the panel knows them), the detected
/// groups and manual sets they were made of included.
fn drop_groups(snapshot: &mut Snapshot, ids: &HashSet<String>) -> usize {
    let before = snapshot.groups.len();
    let gone: Vec<HashSet<i32>> = snapshot.groups.iter().filter(|g| ids.contains(&g.id)).map(|g| g.books.iter().copied().collect()).collect();
    if gone.is_empty() {
        return 0;
    }
    snapshot.detected.retain(|d| !gone.iter().any(|set| d.books.iter().any(|b| set.contains(b))));
    snapshot.manual.retain(|m| !gone.iter().any(|set| m.book_ids.iter().any(|b| set.contains(b))));
    snapshot.derive();
    before - snapshot.groups.len()
}

// ---------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------

pub(crate) fn round2(x: f32) -> f64 {
    ((x as f64) * 100.0).round() / 100.0
}

fn file_mtime(library: &FsPath, rel: &str) -> Option<u64> {
    std::fs::metadata(library.join(rel)).ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}

/// A book as the panel shows it. `with_mtime` stats its files (a page does;
/// the whole-library compatibility report does not).
fn book_json(snap: &Snapshot, id: i32, library: &FsPath, library_id: &str, with_mtime: bool) -> Value {
    book_json_from(&snap.books, id, library, library_id, with_mtime)
}

/// [`book_json`] over any set of loaded books (the compare view's, which are
/// read fresh rather than from the snapshot).
pub fn book_json_from(books: &HashMap<i32, SnapBook>, id: i32, library: &FsPath, library_id: &str, with_mtime: bool) -> Value {
    let Some(b) = books.get(&id) else { return json!({"book_id": id}) };
    let mut newest: Option<u64> = None;
    let files: Vec<Value> = b
        .facts
        .files
        .iter()
        .map(|f| {
            let mtime = if with_mtime { file_mtime(library, &f.path) } else { None };
            newest = newest.max(mtime);
            json!({"format": f.format, "path": f.path, "size": f.size, "mtime": mtime})
        })
        .collect();
    let pdf = b.pdf.map(|p| json!({"pages": p.page_count, "has_text": p.has_text, "scanned": p.scanned, "image_only": !p.has_text && p.scanned, "size": p.size}));
    json!({
        "book_id": id,
        "uuid": b.uuid,
        "title": b.title,
        "authors": b.authors,
        // The count, as before: what the merge ranking read.
        "formats": b.facts.files.len(),
        "format_list": b.facts.files.iter().map(|f| f.format.clone()).collect::<Vec<_>>(),
        "files": files,
        "path": b.facts.folder,
        "size": b.facts.total_size(),
        "mtime": newest,
        "metadata": b.facts.metadata,
        "has_cover": b.facts.has_cover,
        "cover_url": if b.facts.has_cover { Some(if library_id == "default" || library_id == "-" { format!("/get/thumb/{id}") } else { format!("/get/thumb/{id}/{library_id}") }) } else { None },
        "pdf": pdf,
    })
}

pub(crate) fn group_json(snap: &Snapshot, g: &SnapGroup, library: &FsPath, library_id: &str, with_mtime: bool, with_evidence: bool) -> Value {
    let books: Vec<Value> = g.books.iter().map(|id| book_json(snap, *id, library, library_id, with_mtime)).collect();
    let evidence: Vec<Value> = if with_evidence { g.evidence.iter().map(|e| json!({"books": [e.a, e.b], "reason": e.strength.reason(), "confidence": round2(e.confidence), "detail": e.detail})).collect() } else { Vec::new() };
    json!({
        "id": g.id,
        "reason": g.strength.reason(),
        "confidence": round2(g.confidence),
        "size": g.books.len(),
        "books": books,
        "evidence": evidence,
        "suggested_dest": g.keeper.id,
        "keeper_reason": g.keeper.reason,
        "bytes_reclaimable": g.bytes_reclaimable,
        "manual_group_id": g.manual_id,
    })
}

fn facets(snap: &Snapshot, groups: &[&SnapGroup]) -> Value {
    let mut by_reason: BTreeMap<&str, usize> = BTreeMap::new();
    for g in groups {
        *by_reason.entry(g.strength.reason()).or_default() += 1;
    }
    let _ = snap;
    json!({"reason": by_reason})
}

/// What the scan job and the `?job=true` route report: no groups (page them
/// with `GET /duplicates/groups`), the facets and the PDF/empty-file status.
pub fn summary_json(snap: &Snapshot) -> Value {
    let all: Vec<&SnapGroup> = snap.groups.iter().collect();
    json!({
        "generation": snap.generation,
        "watermark": snap.watermark,
        "min": snap.min.reason(),
        "group_count": snap.groups.len(),
        "facets": facets(snap, &all),
        "pdf_unfingerprinted": snap.pdf_status.pending,
        "pdf": snap.pdf_status,
        "empty_files": snap.empty_files,
        "empty_file_count": snap.empty_files.len(),
        "exemptions_applied": snap.exemptions_applied,
    })
}

/// The whole result in one response: the shape `POST /duplicates/scan` has
/// always answered with (groups strongest first, each with evidence), plus
/// the new per-group fields.
pub fn full_report(cache: &Cache, snap: &Snapshot, library_id: &str) -> Value {
    let library = cache.backend.library_path.clone();
    let groups: Vec<Value> = snap.groups.iter().map(|g| group_json(snap, g, &library, library_id, false, true)).collect();
    let mut out = summary_json(snap);
    out["groups"] = Value::Array(groups);
    out
}

// ---------------------------------------------------------------------------
// Paging
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct GroupsQuery {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
    /// Comma-separated reasons (`content,pdf_resave`).
    pub reason: Option<String>,
    pub min_confidence: Option<f64>,
    pub max_confidence: Option<f64>,
    /// Case-insensitive prefix of a file or folder path under the library.
    pub path: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

fn norm_prefix(p: &str) -> String {
    p.replace('\\', "/").trim_matches('/').to_lowercase()
}

fn reasons_of(csv: &str) -> Result<HashSet<&'static str>, ServerError> {
    let mut out = HashSet::new();
    for r in csv.split(',').map(str::trim).filter(|r| !r.is_empty()) {
        if r == "manual" {
            out.insert(Strength::Manual.reason());
            continue;
        }
        let Some(s) = Strength::parse_min(r) else { return Err(ServerError::BadRequest(format!("unknown reason {r:?}"))) };
        // `parse_min` maps "pdf" to the weakest page-content tier, and a
        // reason filter means exactly that tier.
        out.insert(s.reason());
    }
    Ok(out)
}

struct Filter {
    reasons: Option<HashSet<&'static str>>,
    min_conf: Option<f64>,
    max_conf: Option<f64>,
    prefix: Option<String>,
}

impl Filter {
    fn from(q: &GroupsQuery) -> Result<Filter, ServerError> {
        Ok(Filter {
            reasons: match q.reason.as_deref() {
                Some(csv) if !csv.trim().is_empty() => Some(reasons_of(csv)?),
                _ => None,
            },
            min_conf: q.min_confidence,
            max_conf: q.max_confidence,
            prefix: q.path.as_deref().map(norm_prefix).filter(|p| !p.is_empty()),
        })
    }

    fn matches_except_reason(&self, g: &SnapGroup) -> bool {
        let c = round2(g.confidence);
        self.min_conf.map_or(true, |m| c + 1e-9 >= m) && self.max_conf.map_or(true, |m| c - 1e-9 <= m) && self.prefix.as_ref().map_or(true, |p| g.paths_lc.iter().any(|path| path.starts_with(p.as_str())))
    }

}

fn sort_groups(groups: &mut Vec<&SnapGroup>, sort: &str, ascending: bool) {
    groups.sort_by(|a, b| {
        let ord = match sort {
            "size" => b.books.len().cmp(&a.books.len()).then(b.confidence.total_cmp(&a.confidence)),
            "reason" => b.strength.cmp(&a.strength).then(b.confidence.total_cmp(&a.confidence)),
            "reclaimable" => b.bytes_reclaimable.cmp(&a.bytes_reclaimable).then(b.confidence.total_cmp(&a.confidence)),
            _ => b.confidence.total_cmp(&a.confidence).then(b.strength.cmp(&a.strength)),
        };
        let ord = if ascending { ord.reverse() } else { ord };
        ord.then(a.id.cmp(&b.id))
    });
}

/// One page of the snapshot, as the route answers it.
pub fn page_json(cache: &Cache, snap: &Snapshot, q: &GroupsQuery, library_id: &str) -> Result<Value, ServerError> {
    let filter = Filter::from(q)?;
    let sort = q.sort.as_deref().unwrap_or("confidence");
    if !["confidence", "size", "reason", "reclaimable"].contains(&sort) {
        return Err(ServerError::BadRequest(format!("sort must be confidence, size, reason or reclaimable; got {sort:?}")));
    }
    let ascending = match q.order.as_deref() {
        None | Some("desc") => false,
        Some("asc") => true,
        Some(o) => return Err(ServerError::BadRequest(format!("order must be asc or desc; got {o:?}"))),
    };
    let limit = q.limit.unwrap_or(PAGE_DEFAULT).clamp(1, PAGE_MAX);
    let offset = q.offset.unwrap_or(0);

    let without_reason: Vec<&SnapGroup> = snap.groups.iter().filter(|g| filter.matches_except_reason(g)).collect();
    let mut selected: Vec<&SnapGroup> = without_reason.iter().copied().filter(|g| filter.reasons.as_ref().map_or(true, |r| r.contains(g.strength.reason()))).collect();
    sort_groups(&mut selected, sort, ascending);
    let total = selected.len();
    let library = cache.backend.library_path.clone();
    let page: Vec<Value> = selected.iter().skip(offset).take(limit).map(|g| group_json(snap, g, &library, library_id, true, true)).collect();
    let stale = snap.watermark != cache.change_watermark();
    Ok(json!({
        "has_scan": true,
        "stale": stale,
        "generation": snap.generation,
        "scanned_at": snap.created_at,
        "min": snap.min.reason(),
        "total": total,
        "all_total": snap.groups.len(),
        "offset": offset,
        "limit": limit,
        "sort": sort,
        "groups": page,
        "facets": facets(snap, &without_reason),
        "pdf_unfingerprinted": snap.pdf_status.pending,
        "pdf": snap.pdf_status,
        "empty_files": snap.empty_files,
        "empty_file_count": snap.empty_files.len(),
        "exemptions_applied": snap.exemptions_applied,
    }))
}

pub(crate) fn cache_for(state: &AppState, library_id: &str) -> Result<Arc<Cache>, ServerError> {
    state.cache_for(Some(library_id)).ok_or_else(|| ServerError::NotFound(format!("no library named {library_id:?}")))
}

pub(crate) async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, ServerError> + Send + 'static) -> Result<T, ServerError> {
    tokio::task::spawn_blocking(f).await.map_err(|e| ServerError::InternalServerError(e.to_string()))?
}

pub(crate) fn internal(e: anyhow::Error) -> ServerError {
    ServerError::InternalServerError(format!("{e:#}"))
}

/// `GET /duplicates/groups/{library_id}`.
pub async fn groups(State(state): State<AppState>, Path(library_id): Path<String>, Query(query): Query<GroupsQuery>) -> Result<Json<Value>, ServerError> {
    let cache = cache_for(&state, &library_id)?;
    let body = blocking(move || {
        let Some(snap) = snapshot_for(&cache) else { return Ok(json!({"has_scan": false})) };
        page_json(&cache, &snap, &query, &library_id)
    })
    .await?;
    Ok(Json(body))
}

// ---------------------------------------------------------------------------
// Drop, exempt
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct DropBody {
    #[serde(default)]
    pub group_ids: Vec<String>,
    /// Books deleted from their groups: they leave every group, and a group
    /// left with fewer than two books goes too.
    #[serde(default)]
    pub book_ids: Vec<i32>,
    #[serde(default = "default_true")]
    pub adopt: bool,
}

fn remove_books(snapshot: &mut Snapshot, gone: &HashSet<i32>) {
    for g in &mut snapshot.detected {
        g.books.retain(|b| !gone.contains(b));
        g.evidence.retain(|e| !gone.contains(&e.a) && !gone.contains(&e.b));
        if gone.contains(&g.keeper.id) {
            if let Some(&first) = g.books.iter().min() {
                g.keeper = Keeper { id: first, reason: "the suggested copy was removed; the oldest remaining record".to_string() };
            }
        }
    }
    snapshot.detected.retain(|g| g.books.len() >= 2);
    for m in &mut snapshot.manual {
        m.book_ids.retain(|b| !gone.contains(b));
    }
    snapshot.manual.retain(|m| m.book_ids.len() >= 2);
    snapshot.derive();
}

fn default_true() -> bool {
    true
}

/// `POST /duplicates/drop/{library_id}`.
pub async fn drop_route(State(state): State<AppState>, Path(library_id): Path<String>, Json(body): Json<DropBody>) -> Result<Json<Value>, ServerError> {
    let cache = cache_for(&state, &library_id)?;
    let out = blocking(move || {
        let ids: HashSet<String> = body.group_ids.into_iter().collect();
        let mut dropped = 0;
        let gone: HashSet<i32> = body.book_ids.iter().copied().collect();
        update_snapshot(&cache, None, body.adopt, |s| {
            dropped = drop_groups(s, &ids);
            if !gone.is_empty() {
                let before = s.groups.len();
                remove_books(s, &gone);
                dropped += before - s.groups.len();
            }
        });
        let snap = snapshot_for(&cache);
        Ok(json!({"dropped": dropped, "remaining": snap.as_ref().map(|s| s.groups.len()), "stale": snap.as_ref().map(|s| s.watermark != cache.change_watermark())}))
    })
    .await?;
    Ok(Json(out))
}

#[derive(Debug, Deserialize)]
pub struct ExemptBody {
    pub group_id: Option<String>,
    #[serde(default)]
    pub book_ids: Vec<i32>,
}

fn exemption_json(cache: &Cache, e: &dedupe_exemptions::Exemption) -> Value {
    let members: Vec<Value> = e
        .members
        .iter()
        .map(|m| {
            let book_id = cache.book_id_for_uuid(&m.uuid).ok().flatten();
            json!({"uuid": m.uuid, "title": m.title, "authors": m.authors, "book_id": book_id, "present": book_id.is_some()})
        })
        .collect();
    json!({"id": e.id, "created_at": e.created_at, "members": members})
}

/// `POST /duplicates/exempt/{library_id}`.
pub async fn exempt(State(state): State<AppState>, Path(library_id): Path<String>, Json(body): Json<ExemptBody>) -> Result<Json<Value>, ServerError> {
    let cache = cache_for(&state, &library_id)?;
    let out = blocking(move || {
        let mut ids = body.book_ids.clone();
        if let Some(gid) = &body.group_id {
            let snap = snapshot_for(&cache).ok_or_else(|| ServerError::NotFound("no scan to take the group from; scan first or send book_ids".to_string()))?;
            let g = snap.groups.iter().find(|g| &g.id == gid).ok_or_else(|| ServerError::NotFound(format!("no group {gid} in the last scan")))?;
            ids.extend(g.books.iter().copied());
        }
        ids.sort_unstable();
        ids.dedup();
        if ids.len() < 2 {
            return Err(ServerError::BadRequest("need a group_id or at least two book_ids".to_string()));
        }
        for id in &ids {
            if cache.field_for(*id, "path").map_err(|e| ServerError::InternalServerError(e.to_string()))?.is_none() {
                return Err(ServerError::NotFound(format!("no book with id {id}")));
            }
        }
        let before = snapshot_for(&cache).map(|s| s.watermark.clone());
        let ex = dedupe_exemptions::add(&cache, &ids).map_err(internal)?;
        // The newest action wins: books marked "not duplicates" leave a group of the user's own.
        let separated = calibre_db::dedupe_manual_groups::separate(&cache, &ids).map_err(internal)?;
        let manual_after = manual_sets(&cache).map_err(internal)?;
        let set: HashSet<i32> = ids.iter().copied().collect();
        // Groups wholly inside the set vanish. A group that merely overlaps it
        // in two or more books would have to be re-split: that needs a rescan.
        let mut dropped_ids: Vec<String> = Vec::new();
        let mut needs_rescan = false;
        if let Some(snap) = snapshot_for(&cache) {
            for g in &snap.groups {
                let inside = g.books.iter().filter(|b| set.contains(b)).count();
                if inside == g.books.len() {
                    dropped_ids.push(g.id.clone());
                } else if inside >= 2 {
                    needs_rescan = true;
                }
            }
        }
        let drop_set: HashSet<String> = dropped_ids.iter().cloned().collect();
        update_snapshot(&cache, before.as_deref(), !needs_rescan, |s| {
            drop_groups(s, &drop_set);
            s.manual = manual_after;
        });
        let pairs = ids.len() * (ids.len() - 1) / 2;
        Ok(json!({"exemption": exemption_json(&cache, &ex), "pairs": pairs, "dropped_group_ids": dropped_ids, "needs_rescan": needs_rescan, "manual_groups_changed": separated}))
    })
    .await?;
    Ok(Json(out))
}

/// `GET /duplicates/exemptions/{library_id}`.
pub async fn exemptions(State(state): State<AppState>, Path(library_id): Path<String>) -> Result<Json<Value>, ServerError> {
    let cache = cache_for(&state, &library_id)?;
    let out = blocking(move || {
        let list = dedupe_exemptions::list(&cache).map_err(internal)?;
        Ok(json!({"exemptions": list.iter().map(|e| exemption_json(&cache, e)).collect::<Vec<_>>(), "count": list.len()}))
    })
    .await?;
    Ok(Json(out))
}

#[derive(Debug, Deserialize)]
pub struct RemoveExemptionBody {
    pub id: Option<String>,
    #[serde(default)]
    pub all: bool,
}

/// `POST /duplicates/exemptions/remove/{library_id}`. The groups an exemption
/// hid come back with the next scan, so the response says `needs_rescan`.
pub async fn remove_exemption(State(state): State<AppState>, Path(library_id): Path<String>, Json(body): Json<RemoveExemptionBody>) -> Result<Json<Value>, ServerError> {
    let cache = cache_for(&state, &library_id)?;
    let out = blocking(move || {
        let removed = if body.all {
            dedupe_exemptions::clear(&cache).map_err(internal)?
        } else {
            let id = body.id.as_deref().ok_or_else(|| ServerError::BadRequest("send {id} or {all: true}".to_string()))?;
            dedupe_exemptions::remove(&cache, id).map_err(internal)? as usize
        };
        Ok(json!({"removed": removed, "needs_rescan": removed > 0}))
    })
    .await?;
    Ok(Json(out))
}

// ---------------------------------------------------------------------------
// Resolve
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ResolveBody {
    #[serde(default = "default_true")]
    pub dry_run: bool,
    #[serde(default)]
    pub action: ResolveAction,
    /// Reasons to include. Default `["content"]`.
    pub tiers: Option<Vec<String>>,
    pub min_confidence: Option<f64>,
    pub path_prefix: Option<String>,
    /// Explicit groups; overrides `tiers`, `min_confidence` and `path_prefix`.
    pub group_ids: Option<Vec<String>>,
    /// `{group_id: book_id}` keeper overrides.
    #[serde(default)]
    pub keepers: HashMap<String, i32>,
    #[serde(default)]
    pub keep_duplicate_formats: bool,
    pub plan_fingerprint: Option<String>,
}

/// What the resolve job runs.
#[derive(Clone)]
pub struct ResolveJob {
    pub run_id: String,
    pub action: ResolveAction,
    pub plan: Vec<PlannedResolve>,
    pub keep_duplicate_formats: bool,
    /// The snapshot watermark the plan was made against.
    pub watermark: String,
}

struct Planned {
    plan: Vec<PlannedResolve>,
    groups_json: Vec<Value>,
    skipped: Vec<Value>,
    missing: Vec<String>,
}

fn plan_from_snapshot(snap: &Snapshot, body: &ResolveBody, library_id: &str, library: &FsPath) -> Result<Planned, ServerError> {
    let selected: Vec<&SnapGroup> = match &body.group_ids {
        Some(ids) => {
            let wanted: HashSet<&str> = ids.iter().map(String::as_str).collect();
            snap.groups.iter().filter(|g| wanted.contains(g.id.as_str())).collect()
        }
        None => {
            let tiers = match &body.tiers {
                Some(t) => reasons_of(&t.join(","))?,
                None => HashSet::from(["content"]),
            };
            let prefix = body.path_prefix.as_deref().map(norm_prefix).filter(|p| !p.is_empty());
            snap.groups
                .iter()
                .filter(|g| tiers.contains(g.strength.reason()))
                .filter(|g| body.min_confidence.map_or(true, |m| round2(g.confidence) + 1e-9 >= m))
                .filter(|g| prefix.as_ref().map_or(true, |p| g.paths_lc.iter().any(|path| path.starts_with(p.as_str()))))
                .collect()
        }
    };
    let missing: Vec<String> = body.group_ids.iter().flatten().filter(|id| !snap.groups.iter().any(|g| &g.id == *id)).cloned().collect();
    for (gid, keeper) in &body.keepers {
        if let Some(g) = snap.groups.iter().find(|g| &g.id == gid) {
            if !g.books.contains(keeper) {
                return Err(ServerError::BadRequest(format!("book {keeper} is not in group {gid}")));
            }
        }
    }

    let mut plan = Vec::new();
    let mut groups_json = Vec::new();
    let mut skipped = Vec::new();
    for g in selected {
        let overridden = body.keepers.get(&g.id).copied();
        let (keeper, reason) = match overridden {
            Some(k) if k != g.keeper.id => (k, "chosen by you".to_string()),
            _ => (g.keeper.id, g.keeper.reason.clone()),
        };
        let others: Vec<i32> = g.books.iter().copied().filter(|b| *b != keeper).collect();
        let keeper_facts = snap.books.get(&keeper).map(|b| b.facts.clone()).unwrap_or_default();
        if !keeper_facts.files.iter().any(|f| f.size > 0) {
            skipped.push(json!({"group_id": g.id, "reason": "the keeper has no file on disk"}));
            continue;
        }
        if others.is_empty() {
            continue;
        }
        let other_facts: Vec<&BookFacts> = others.iter().filter_map(|id| snap.books.get(id).map(|b| &b.facts)).collect();
        let bytes_freed = resolve::bytes_freed(body.action, &keeper_facts, &other_facts);
        groups_json.push(json!({
            "group_id": g.id,
            "reason": g.strength.reason(),
            "confidence": round2(g.confidence),
            "action": body.action.name(),
            "keeper": book_json(snap, keeper, library, library_id, false),
            "keeper_reason": reason,
            "others": others.iter().map(|id| book_json(snap, *id, library, library_id, false)).collect::<Vec<_>>(),
            "bytes_freed": bytes_freed,
        }));
        plan.push(PlannedResolve { group_id: g.id.clone(), keeper, keeper_reason: reason, others, bytes_freed });
    }
    Ok(Planned { plan, groups_json, skipped, missing })
}

fn conflict(body: Value) -> Response {
    (StatusCode::CONFLICT, Json(body)).into_response()
}

/// What a resolve request turned into.
enum Resolved {
    /// Answer with this.
    Answer(StatusCode, Value),
    /// Start this job.
    Start(Box<ResolveJob>),
}

fn resolve_core(cache: &Cache, library_id: &str, body: &ResolveBody) -> Result<Resolved, ServerError> {
    let snap = snapshot_for(cache).ok_or_else(|| ServerError::BadRequest("scan the library first: there is no duplicate scan to plan from".to_string()))?;
    let watermark = cache.change_watermark();
    if snap.watermark != watermark {
        return Ok(Resolved::Answer(StatusCode::CONFLICT, json!({"error": "the library has changed since the last scan; rescan before resolving", "stale": true})));
    }
    let planned = plan_from_snapshot(&snap, body, library_id, &cache.backend.library_path)?;
    let fingerprint = resolve::plan_fingerprint(body.action, &planned.plan, &snap.watermark);

    if body.dry_run {
        let books: usize = planned.plan.iter().map(|g| g.others.len()).sum();
        let bytes: i64 = planned.plan.iter().map(|g| g.bytes_freed).sum();
        let (restorable, note) = match body.action {
            ResolveAction::KeepBestTrashRest => (true, "The other books are moved to the library trash (.caltrash) and can be restored, as a whole run or one by one, until the trash expires."),
            ResolveAction::MergeAsFormats => (false, "Each other book is merged into the keeper (its formats, tags, identifiers and other missing fields are copied in) and then trashed. Restoring the run brings the trashed books back, but does NOT remove what was merged into the keeper."),
        };
        return Ok(Resolved::Answer(
            StatusCode::OK,
            json!({
                "dry_run": true,
                "action": body.action.name(),
                "plan_fingerprint": fingerprint,
                "groups": planned.groups_json,
                "totals": {"groups": planned.plan.len(), "books": books, "bytes_freed": bytes},
                "skipped": planned.skipped,
                "missing_group_ids": planned.missing,
                "fully_restorable": restorable,
                "note": note,
            }),
        ));
    }

    let Some(confirmed) = body.plan_fingerprint.as_deref() else {
        return Err(ServerError::BadRequest("executing needs the plan_fingerprint of the dry run the user confirmed".to_string()));
    };
    if confirmed != fingerprint {
        return Ok(Resolved::Answer(StatusCode::CONFLICT, json!({"error": "the plan is no longer what was previewed (the library or the selection changed); run the dry run again", "plan_fingerprint": fingerprint})));
    }
    if planned.plan.is_empty() {
        return Err(ServerError::BadRequest("the plan has no groups to resolve".to_string()));
    }
    Ok(Resolved::Start(Box::new(ResolveJob { run_id: resolve::new_run_id(), action: body.action, plan: planned.plan, keep_duplicate_formats: body.keep_duplicate_formats, watermark: snap.watermark.clone() })))
}

/// `POST /duplicates/resolve/{library_id}`.
pub async fn resolve_route(State(state): State<AppState>, Path(library_id): Path<String>, Json(body): Json<ResolveBody>) -> Result<Response, ServerError> {
    let cache = cache_for(&state, &library_id)?;
    let resolved = {
        let cache = cache.clone();
        blocking(move || resolve_core(&cache, &library_id, &body)).await?
    };
    match resolved {
        Resolved::Answer(status, body) => Ok((status, Json(body)).into_response()),
        Resolved::Start(job) => {
            let run_id = job.run_id.clone();
            let params = JobParams { resolve: Some(*job), ..Default::default() };
            Ok(match library_jobs::start_with(cache, JobKind::Resolve, None, params) {
                Started::New(job) => (StatusCode::ACCEPTED, Json(json!({"job_id": job.id, "run_id": run_id, "state": "running", "already_running": false, "status_url": format!("/library-jobs/{}", job.id)}))).into_response(),
                Started::AlreadyRunning(job) => conflict(json!({"error": "a resolve job is already running on this library", "job_id": job.id})),
                Started::Busy(active) => conflict(json!({"error": format!("a {} job is already running on this library", active.kind.name()), "job_id": active.id})),
            })
        }
    }
}

/// What the `resolve` library job runs: the plan, then the snapshot is
/// brought up to date (the groups that were resolved leave it, and it stays
/// current if only this run changed the library).
pub fn run_resolve_job(cache: &Cache, job: &ResolveJob, control: &JobControl) -> anyhow::Result<Value> {
    let report = resolve::execute_resolve(cache, &job.run_id, job.action, &job.plan, job.keep_duplicate_formats, control);
    let done: HashSet<String> = report.done_groups.iter().cloned().collect();
    // A resolved group of the user's own has fewer than two live books left: it goes.
    let _ = calibre_db::dedupe_manual_groups::prune(cache);
    let manual_after = manual_sets(cache)?;
    update_snapshot(cache, Some(&job.watermark), true, |s| {
        drop_groups(s, &done);
        s.manual = manual_after;
    });
    let mut out = serde_json::to_value(&report)?;
    out["fully_restorable"] = json!(job.action == ResolveAction::KeepBestTrashRest);
    Ok(out)
}

/// `GET /duplicates/runs/{library_id}` -- resolve runs that still have books in the trash.
pub async fn runs(State(state): State<AppState>, Path(library_id): Path<String>) -> Result<Json<Value>, ServerError> {
    let cache = cache_for(&state, &library_id)?;
    let out = blocking(move || {
        let runs = cache.trash_runs().map_err(internal)?;
        Ok(json!({"runs": runs}))
    })
    .await?;
    Ok(Json(out))
}

/// `POST /duplicates/runs/{library_id}/{run_id}/restore` -- puts back every
/// book the run trashed. The snapshot goes stale (the library changed), so the
/// response says `needs_rescan`.
pub async fn restore_run(State(state): State<AppState>, Path((library_id, run_id)): Path<(String, String)>) -> Result<Json<Value>, ServerError> {
    let cache = cache_for(&state, &library_id)?;
    crate::library_jobs::ensure_no_file_mover(&cache)?;
    let out = blocking(move || {
        if cache.trash_entries_for_run(&run_id).map_err(internal)?.is_empty() {
            return Err(ServerError::NotFound(format!("nothing in the trash belongs to run {run_id}")));
        }
        let restored = cache.restore_trash_run(&run_id).map_err(internal)?;
        Ok(json!({"run_id": run_id, "restored": restored.restored, "failed": restored.failed, "needs_rescan": !restored.restored.is_empty()}))
    })
    .await?;
    Ok(Json(out))
}



#[cfg(test)]
#[path = "duplicates_review_tests.rs"]
mod tests;
