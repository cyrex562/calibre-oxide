//! Resolving duplicate groups in bulk (tranche 12): choose which copy to
//! keep, and either send the rest to the library trash or merge them into it.
//!
//! The *grouping* is [`crate::dedupe`]; this module is what happens after a
//! person has reviewed the groups (or told the app which tiers to trust).
//!
//! # Keeper heuristic
//!
//! [`choose_keeper`] ranks a group's books and says **why** the winner won
//! (the first criterion on which it beats the runner-up). A book with no file
//! on disk never beats one that has one.
//!
//! * **PDF groups** (a page-content tier, or every member fingerprinted):
//!   a text layer, then not an image-only scan, then more pages, then the
//!   larger file (an optimizer shrinks images, so of two re-saves the larger
//!   is the original), then the richer record, then the lowest id.
//! * **Everything else**: more formats, then the richer record (more filled
//!   fields), then has a cover, then the better format (`EPUB` over `MOBI`
//!   over `PDF` over `TXT`, see [`format_quality`]), then more bytes, then the
//!   lowest id (the oldest record).
//!
//! # Actions and what can be undone
//!
//! * [`ResolveAction::KeepBestTrashRest`]: every other book is deleted *with
//!   its files into the library's `.caltrash`* (`crate::trash`). Fully
//!   undoable until the trash expires: `restore_trash_run` puts back every
//!   book the run trashed, with its uuid and (when free) its id.
//! * [`ResolveAction::MergeAsFormats`]: the merge engine
//!   ([`crate::merge`], mode `merge_and_delete`) folds each other book's
//!   formats, tags, identifiers, comments... into the keeper and trashes the
//!   source record. **Restoring the run brings the trashed source books back
//!   but does not take the formats and metadata out of the keeper again**, so
//!   the keeper then holds a copy of what the restored book holds. Hence the
//!   bulk resolve always dry-runs first (see the server route).
//!
//! Every book a run trashes is tagged with the run's id in its trash manifest
//! ([`crate::trash::with_trash_run`]).

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::cache::Cache;
use crate::dedupe::{BookFacts, PdfBookInfo};
use crate::merge::{MergeMode, MergeOptions};
use crate::scan_pipeline::JobControl;

/// What a resolve run does with the books that are not the keeper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ResolveAction {
    /// Trash the others (restorable).
    #[default]
    KeepBestTrashRest,
    /// Merge the others into the keeper, then trash them (only the trashing
    /// is restorable).
    MergeAsFormats,
}

impl ResolveAction {
    pub fn name(self) -> &'static str {
        match self {
            ResolveAction::KeepBestTrashRest => "keep_best_trash_rest",
            ResolveAction::MergeAsFormats => "merge_as_formats",
        }
    }
}

/// How good a format is to keep, higher is better.
pub fn format_quality(format: &str) -> u32 {
    match format.to_ascii_lowercase().as_str() {
        "epub" | "kepub" => 100,
        "azw3" => 95,
        "mobi" => 80,
        "azw" => 75,
        "fb2" => 70,
        "docx" => 60,
        "pdf" => 55,
        "lit" => 50,
        "rtf" | "odt" => 40,
        "htm" | "html" | "htmlz" | "xhtml" => 35,
        "txt" | "txtz" => 30,
        "djvu" => 25,
        "cbz" | "cbr" | "cb7" => 20,
        _ => 10,
    }
}

/// A book a group might keep, with what the ranking reads.
#[derive(Debug, Clone)]
pub struct KeeperCandidate {
    pub id: i32,
    pub facts: BookFacts,
    /// Present for a fingerprinted PDF.
    pub pdf: Option<PdfBookInfo>,
}

/// The chosen copy and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Keeper {
    pub id: i32,
    pub reason: String,
}

/// One ranking criterion: a name for the reason, a score, and how to say it.
struct Criterion {
    score: fn(&KeeperCandidate) -> i64,
    why: fn(&KeeperCandidate, &KeeperCandidate) -> String,
}

fn has_files(c: &KeeperCandidate) -> bool {
    c.facts.files.iter().any(|f| f.size > 0)
}

fn best_quality(c: &KeeperCandidate) -> (u32, String) {
    c.facts.files.iter().map(|f| (format_quality(&f.format), f.format.clone())).max().unwrap_or((0, String::new()))
}

fn mb(bytes: i64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_048_576.0)
}

fn pdf_criteria() -> Vec<Criterion> {
    vec![
        Criterion { score: |c| has_files(c) as i64, why: |_, _| "it is the only copy with a file on disk".into() },
        Criterion { score: |c| c.pdf.is_some_and(|p| p.has_text) as i64, why: |_, _| "it has a text layer (the other copy is image-only)".into() },
        Criterion { score: |c| c.pdf.is_some_and(|p| !(p.scanned && !p.has_text)) as i64, why: |_, _| "the other copy is an image-only scan".into() },
        Criterion { score: |c| c.pdf.map(|p| p.page_count as i64).unwrap_or(0), why: |w, r| format!("more pages ({} vs {})", w.pdf.map(|p| p.page_count).unwrap_or(0), r.pdf.map(|p| p.page_count).unwrap_or(0)) },
        Criterion {
            score: |c| c.pdf.map(|p| p.size).unwrap_or_else(|| c.facts.total_size()),
            why: |w, r| format!("larger file ({} vs {}; an optimizer shrinks images)", mb(w.pdf.map(|p| p.size).unwrap_or_else(|| w.facts.total_size())), mb(r.pdf.map(|p| p.size).unwrap_or_else(|| r.facts.total_size()))),
        },
        Criterion { score: |c| c.facts.metadata as i64, why: |w, r| format!("richer record ({} vs {} fields)", w.facts.metadata, r.facts.metadata) },
        Criterion { score: |c| c.facts.has_cover as i64, why: |_, _| "it has a cover".into() },
    ]
}

fn general_criteria() -> Vec<Criterion> {
    vec![
        Criterion { score: |c| has_files(c) as i64, why: |_, _| "it is the only copy with a file on disk".into() },
        Criterion { score: |c| c.facts.files.len() as i64, why: |w, r| format!("more formats ({} vs {})", w.facts.files.len(), r.facts.files.len()) },
        Criterion { score: |c| c.facts.metadata as i64, why: |w, r| format!("richer record ({} vs {} fields)", w.facts.metadata, r.facts.metadata) },
        Criterion { score: |c| c.facts.has_cover as i64, why: |_, _| "it has a cover".into() },
        Criterion { score: |c| best_quality(c).0 as i64, why: |w, r| format!("better format ({} over {})", best_quality(w).1, best_quality(r).1) },
        Criterion { score: |c| c.facts.total_size(), why: |w, r| format!("larger files ({} vs {})", mb(w.facts.total_size()), mb(r.facts.total_size())) },
    ]
}

/// The copy of a group to keep, and the reason. `pdf_group` is true when the
/// group was found from page content (a scan and its OCR, an optimizer's
/// re-save); a group whose members are all fingerprinted PDFs is ranked the
/// same way. `None` for an empty group.
pub fn choose_keeper(pdf_group: bool, candidates: &[KeeperCandidate]) -> Option<Keeper> {
    if candidates.is_empty() {
        return None;
    }
    let pdf_mode = pdf_group || candidates.iter().all(|c| c.pdf.is_some());
    let criteria = if pdf_mode { pdf_criteria() } else { general_criteria() };
    let key = |c: &KeeperCandidate| -> Vec<i64> { criteria.iter().map(|k| (k.score)(c)).collect() };
    // Highest key wins; the lowest id breaks every remaining tie.
    let mut ranked: Vec<&KeeperCandidate> = candidates.iter().collect();
    ranked.sort_by(|a, b| key(b).cmp(&key(a)).then(a.id.cmp(&b.id)));
    let winner = ranked[0];
    let Some(runner_up) = ranked.get(1) else { return Some(Keeper { id: winner.id, reason: "the only book".to_string() }) };
    let (wk, rk) = (key(winner), key(runner_up));
    let reason = match criteria.iter().enumerate().find(|(i, _)| wk[*i] > rk[*i]) {
        Some((_, k)) => (k.why)(winner, runner_up),
        None => "the copies are equivalent; kept the oldest record (lowest id)".to_string(),
    };
    Some(Keeper { id: winner.id, reason })
}

/// One group's part of a plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedResolve {
    pub group_id: String,
    pub keeper: i32,
    pub keeper_reason: String,
    /// The books to trash (or merge in), ascending.
    pub others: Vec<i32>,
    /// Bytes the others' files hold that the keeper does not need.
    pub bytes_freed: i64,
}

/// A fingerprint of a plan and of the library state it was made for, so an
/// execute request is refused when either changed since the user saw it.
pub fn plan_fingerprint(action: ResolveAction, plan: &[PlannedResolve], watermark: &str) -> String {
    let mut h = blake3::Hasher::new();
    h.update(action.name().as_bytes());
    h.update(watermark.as_bytes());
    for g in plan {
        h.update(format!("{}:{}<-{:?};", g.group_id, g.keeper, g.others).as_bytes());
    }
    h.finalize().to_hex().to_string()[..16].to_string()
}

/// A fresh run id: `run-<unix seconds>-<4 hex>`.
pub fn new_run_id() -> String {
    format!("run-{}-{}", crate::trash::now_secs(), &uuid::Uuid::new_v4().simple().to_string()[..4])
}

/// What a run did.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ResolveReport {
    pub run_id: String,
    pub action: String,
    pub groups_planned: usize,
    pub groups_done: usize,
    /// Ids of the groups that completed without error.
    pub done_groups: Vec<String>,
    pub books_trashed: usize,
    pub bytes_freed: i64,
    /// Files the run moved into the library trash.
    pub trashed_files: Vec<String>,
    /// `(group id, why)` for groups skipped or that failed in whole or part.
    pub errors: Vec<(String, String)>,
    pub cancelled: bool,
}

/// Runs `plan` group by group, tagging everything trashed with `run_id`, and
/// stopping between groups when `control` is cancelled. A group that fails is
/// recorded and the next one runs. Each group is re-checked first: the keeper
/// must still exist and still have a file on disk, or nothing in the group is
/// touched.
pub fn execute_resolve(cache: &Cache, run_id: &str, action: ResolveAction, plan: &[PlannedResolve], keep_duplicate_formats: bool, control: &JobControl) -> ResolveReport {
    let mut out = ResolveReport { run_id: run_id.to_string(), action: action.name().to_string(), groups_planned: plan.len(), ..Default::default() };
    control.total.store(plan.len() as u64, Ordering::Relaxed);
    control.set_phase("resolving");
    for group in plan {
        if control.is_cancelled() {
            out.cancelled = true;
            break;
        }
        match resolve_group(cache, run_id, action, group, keep_duplicate_formats) {
            Ok(done) => {
                out.books_trashed += done.trashed_books;
                out.trashed_files.extend(done.files);
                if done.errors.is_empty() {
                    out.groups_done += 1;
                    out.done_groups.push(group.group_id.clone());
                    out.bytes_freed += group.bytes_freed;
                } else {
                    control.errors.fetch_add(1, Ordering::Relaxed);
                }
                out.errors.extend(done.errors.into_iter().map(|e| (group.group_id.clone(), e)));
            }
            Err(e) => {
                out.errors.push((group.group_id.clone(), format!("{e:#}")));
                control.errors.fetch_add(1, Ordering::Relaxed);
            }
        }
        control.seen.fetch_add(1, Ordering::Relaxed);
    }
    out
}

#[derive(Default)]
struct GroupDone {
    trashed_books: usize,
    files: Vec<String>,
    errors: Vec<String>,
}

fn has_file_on_disk(cache: &Cache, book: i32) -> Result<bool> {
    Ok(cache.format_file_info(book)?.iter().any(|f| f.path.is_file()))
}

fn resolve_group(cache: &Cache, run_id: &str, action: ResolveAction, group: &PlannedResolve, keep_duplicate_formats: bool) -> Result<GroupDone> {
    let mut done = GroupDone::default();
    if cache.field_for(group.keeper, "path")?.is_none() {
        done.errors.push(format!("the keeper (book {}) is no longer in the library; nothing was changed", group.keeper));
        return Ok(done);
    }
    if !has_file_on_disk(cache, group.keeper)? {
        done.errors.push(format!("the keeper (book {}) has no file on disk; nothing was changed", group.keeper));
        return Ok(done);
    }
    let others: Vec<i32> = group.others.iter().copied().filter(|id| *id != group.keeper).collect();
    match action {
        ResolveAction::KeepBestTrashRest => {
            for id in others {
                if cache.field_for(id, "path")?.is_none() {
                    continue; // already gone
                }
                match crate::trash::with_trash_run(run_id, || crate::removal::delete_with_file(cache, id)) {
                    Ok(report) => {
                        done.trashed_books += 1;
                        done.files.extend(report.deleted);
                        for (path, why) in report.failed {
                            done.errors.push(format!("book {id}: {path} could not be moved to the trash: {why}"));
                        }
                    }
                    Err(e) => done.errors.push(format!("book {id}: {e:#}")),
                }
            }
        }
        ResolveAction::MergeAsFormats => {
            let live: Vec<i32> = others.into_iter().filter(|id| cache.field_for(*id, "path").ok().flatten().is_some()).collect();
            if live.is_empty() {
                return Ok(done);
            }
            let opts = MergeOptions { mode: MergeMode::MergeAndDelete, keep_duplicate_formats, dry_run: false };
            let report = crate::trash::with_trash_run(run_id, || crate::merge::merge_books(cache, group.keeper, &live, &opts))?;
            done.files.extend(report.trashed());
            for s in &report.sources {
                if let Some(e) = &s.error {
                    done.errors.push(format!("book {}: {e}", s.book_id));
                } else if s.deleted {
                    done.trashed_books += 1;
                }
            }
        }
    }
    Ok(done)
}

/// Bytes of `others`' files that the keeper does not need under `action`:
/// all of them when trashing, and only the formats the keeper already holds
/// when merging (the rest are copied into the keeper, not freed).
pub fn bytes_freed(action: ResolveAction, keeper: &BookFacts, others: &[&BookFacts]) -> i64 {
    match action {
        ResolveAction::KeepBestTrashRest => others.iter().map(|o| o.total_size()).sum(),
        ResolveAction::MergeAsFormats => {
            let have: std::collections::HashSet<&str> = keeper.files.iter().map(|f| f.format.as_str()).collect();
            others.iter().flat_map(|o| o.files.iter()).filter(|f| have.contains(f.format.as_str())).map(|f| f.size.max(0)).sum()
        }
    }
}

/// `facts` keyed by id, for building candidates.
pub fn candidates(ids: &[i32], facts: &HashMap<i32, BookFacts>, pdf: &HashMap<i32, PdfBookInfo>) -> Vec<KeeperCandidate> {
    ids.iter().map(|&id| KeeperCandidate { id, facts: facts.get(&id).cloned().unwrap_or(BookFacts { id, ..Default::default() }), pdf: pdf.get(&id).copied() }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dedupe::BookFile;

    fn facts(id: i32, files: &[(&str, i64)], metadata: u32, has_cover: bool) -> BookFacts {
        BookFacts { id, folder: String::new(), files: files.iter().map(|(f, s)| BookFile { format: f.to_string(), path: format!("{id}.{}", f.to_lowercase()), size: *s }).collect(), metadata, has_cover }
    }

    fn cand(id: i32, files: &[(&str, i64)], metadata: u32, cover: bool, pdf: Option<PdfBookInfo>) -> KeeperCandidate {
        KeeperCandidate { id, facts: facts(id, files, metadata, cover), pdf }
    }

    fn pdf(pages: u32, has_text: bool, scanned: bool, size: i64) -> Option<PdfBookInfo> {
        Some(PdfBookInfo { page_count: pages, has_text, scanned, size })
    }

    #[test]
    fn a_pdf_with_a_text_layer_beats_the_scan_even_if_the_scan_is_bigger() {
        let scan = cand(1, &[("PDF", 90_000_000)], 5, true, pdf(300, false, true, 90_000_000));
        let ocr = cand(2, &[("PDF", 20_000_000)], 0, false, pdf(300, true, true, 20_000_000));
        let k = choose_keeper(true, &[scan, ocr]).unwrap();
        assert_eq!(k.id, 2);
        assert!(k.reason.contains("text layer"), "{}", k.reason);
    }

    #[test]
    fn without_text_a_scanned_pdf_loses_to_one_that_is_not_an_image_scan() {
        let scan = cand(1, &[("PDF", 10)], 0, false, pdf(10, false, true, 10));
        let other = cand(2, &[("PDF", 5)], 0, false, pdf(10, false, false, 5));
        let k = choose_keeper(true, &[scan, other]).unwrap();
        assert_eq!(k.id, 2);
        assert!(k.reason.contains("image-only"), "{}", k.reason);
    }

    #[test]
    fn of_two_resaves_the_one_with_more_pages_then_the_larger_file_wins() {
        let a = cand(1, &[("PDF", 10)], 0, false, pdf(100, true, false, 10));
        let b = cand(2, &[("PDF", 10)], 0, false, pdf(120, true, false, 10));
        let k = choose_keeper(true, &[a.clone(), b]).unwrap();
        assert_eq!((k.id, k.reason.starts_with("more pages")), (2, true), "{}", k.reason);
        let big = cand(3, &[("PDF", 50)], 0, false, pdf(100, true, false, 50));
        let k = choose_keeper(true, &[a, big]).unwrap();
        assert_eq!(k.id, 3);
        assert!(k.reason.starts_with("larger file"), "{}", k.reason);
    }

    #[test]
    fn a_non_pdf_group_prefers_formats_then_the_richer_record_then_format_quality_then_size() {
        // More formats beats everything below it.
        let two = cand(5, &[("EPUB", 1), ("MOBI", 1)], 0, false, None);
        let one = cand(6, &[("EPUB", 1)], 7, true, None);
        let k = choose_keeper(false, &[one.clone(), two]).unwrap();
        assert_eq!(k.id, 5);
        assert!(k.reason.starts_with("more formats"), "{}", k.reason);
        // Same formats: the richer record.
        let rich = cand(8, &[("EPUB", 1)], 6, false, None);
        let k = choose_keeper(false, &[one.clone(), rich]).unwrap();
        assert_eq!(k.id, 6, "7 fields beat 6");
        assert!(k.reason.starts_with("richer record"), "{}", k.reason);
        // Equal records: the better format, then the larger file, then the lowest id.
        let epub = cand(10, &[("EPUB", 1)], 2, false, None);
        let txt = cand(9, &[("TXT", 99)], 2, false, None);
        let k = choose_keeper(false, &[txt, epub]).unwrap();
        assert_eq!(k.id, 10);
        assert!(k.reason.starts_with("better format"), "{}", k.reason);
        let small = cand(11, &[("EPUB", 1)], 2, false, None);
        let large = cand(12, &[("EPUB", 5)], 2, false, None);
        let k = choose_keeper(false, &[small.clone(), large]).unwrap();
        assert_eq!(k.id, 12);
        assert!(k.reason.starts_with("larger files"), "{}", k.reason);
        let twin = cand(4, &[("EPUB", 1)], 2, false, None);
        let k = choose_keeper(false, &[small, twin]).unwrap();
        assert_eq!(k.id, 4, "oldest record");
        assert!(k.reason.contains("lowest id"), "{}", k.reason);
    }

    #[test]
    fn a_book_with_no_file_never_wins() {
        let empty = cand(1, &[], 9, true, None);
        let plain = cand(2, &[("TXT", 3)], 0, false, None);
        assert_eq!(choose_keeper(false, &[empty, plain]).unwrap().id, 2);
        assert_eq!(choose_keeper(false, &[]), None);
    }

    // ---- executing ----

    fn add_book(dir: &std::path::Path, cache: &Cache, name: &str, title: &str, bytes: &[u8]) -> i32 {
        let source = dir.join(name);
        std::fs::write(&source, bytes).unwrap();
        let mut meta = calibre_ebooks::metadata::MetaInformation::default();
        meta.title = title.to_string();
        meta.authors = vec!["Ada Lovelace".to_string()];
        cache.add_book(&source, &meta).unwrap()
    }

    fn plan(keeper: i32, others: &[i32]) -> Vec<PlannedResolve> {
        vec![PlannedResolve { group_id: "g1".into(), keeper, keeper_reason: "test".into(), others: others.to_vec(), bytes_freed: 7 }]
    }

    #[test]
    fn keep_best_trash_rest_trashes_the_others_under_a_run_id_and_the_run_restores() {
        let lib = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let cache = Cache::new(lib.path()).unwrap();
        let keeper = add_book(src.path(), &cache, "k.txt", "Keep", b"keeper bytes");
        let x = add_book(src.path(), &cache, "x.txt", "Extra X", b"x bytes");
        let y = add_book(src.path(), &cache, "y.txt", "Extra Y", b"y bytes");
        let x_uuid = cache.book_uuid(x).unwrap().unwrap();

        let control = JobControl::new();
        let report = execute_resolve(&cache, "run-test-1", ResolveAction::KeepBestTrashRest, &plan(keeper, &[x, y]), false, &control);
        assert!(report.errors.is_empty(), "{report:?}");
        assert_eq!((report.groups_done, report.books_trashed, report.bytes_freed), (1, 2, 7));
        assert_eq!(cache.all_book_ids().unwrap(), vec![keeper]);
        assert!(!report.trashed_files.is_empty());

        let entries = cache.trash_entries_for_run("run-test-1").unwrap();
        assert_eq!(entries.len(), 2);
        assert!(cache.trash_entries_for_run("run-other").unwrap().is_empty());
        let runs = cache.trash_runs().unwrap();
        assert_eq!((runs.len(), runs[0].run_id.as_str(), runs[0].books), (1, "run-test-1", 2));

        // An ordinary delete is not part of any run.
        let z = add_book(src.path(), &cache, "z.txt", "Zed", b"z bytes");
        crate::removal::delete_with_file(&cache, z).unwrap();
        assert_eq!(cache.trash_entries_for_run("run-test-1").unwrap().len(), 2);

        let restored = cache.restore_trash_run("run-test-1").unwrap();
        assert!(restored.failed.is_empty(), "{restored:?}");
        assert_eq!(restored.restored.len(), 2);
        assert!(cache.book_id_for_uuid(&x_uuid).unwrap().is_some(), "the same uuid is back");
        assert_eq!(cache.all_book_ids().unwrap().len(), 3);
        assert!(cache.trash_entries_for_run("run-test-1").unwrap().is_empty());
        // The unrelated trashed book is still in the trash.
        assert_eq!(cache.list_trash().unwrap().len(), 1);
    }

    #[test]
    fn a_keeper_without_a_file_leaves_the_group_untouched() {
        let lib = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let cache = Cache::new(lib.path()).unwrap();
        let keeper = add_book(src.path(), &cache, "k.txt", "Keep", b"keeper bytes");
        let x = add_book(src.path(), &cache, "x.txt", "Extra X", b"x bytes");
        let file = cache.format_file_info(keeper).unwrap()[0].path.clone();
        std::fs::remove_file(file).unwrap();
        let report = execute_resolve(&cache, "run-test-2", ResolveAction::KeepBestTrashRest, &plan(keeper, &[x]), false, &JobControl::new());
        assert_eq!((report.groups_done, report.books_trashed), (0, 0));
        assert_eq!(report.errors.len(), 1, "{report:?}");
        assert_eq!(cache.all_book_ids().unwrap().len(), 2);
    }

    #[test]
    fn merge_as_formats_folds_the_other_into_the_keeper_and_trashes_it_under_the_run() {
        let lib = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let cache = Cache::new(lib.path()).unwrap();
        // Separate folders, so the source's file is its own and goes to the trash.
        let _ = src;
        let register = |rel: &str, bytes: &[u8]| {
            let path = lib.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, bytes).unwrap();
            let mut meta = calibre_ebooks::metadata::MetaInformation::default();
            meta.title = "Keep".to_string();
            meta.authors = vec!["Ada Lovelace".to_string()];
            cache.register_book_in_place(rel, &meta).unwrap()
        };
        let keeper = register("A/k.txt", b"keeper bytes");
        let other = register("B/o.epub", b"epub bytes");
        cache.set_field(other, "tags", "fiction").unwrap();

        let report = execute_resolve(&cache, "run-test-3", ResolveAction::MergeAsFormats, &plan(keeper, &[other]), false, &JobControl::new());
        assert!(report.errors.is_empty(), "{report:?}");
        assert_eq!((report.groups_done, report.books_trashed), (1, 1));
        assert_eq!(cache.all_book_ids().unwrap(), vec![keeper]);
        let mut formats: Vec<String> = cache.format_file_info(keeper).unwrap().into_iter().map(|f| f.fmt).collect();
        formats.sort();
        assert_eq!(formats, vec!["EPUB".to_string(), "TXT".to_string()]);
        assert_eq!(cache.field_for(keeper, "tags").unwrap().as_deref(), Some("fiction"));
        // The merged source went to the trash tagged with the run.
        assert_eq!(cache.trash_entries_for_run("run-test-3").unwrap().len(), 1);

        // Restoring brings the source back, but the keeper keeps what was merged in.
        let restored = cache.restore_trash_run("run-test-3").unwrap();
        assert_eq!(restored.restored.len(), 1, "{restored:?}");
        assert_eq!(cache.all_book_ids().unwrap().len(), 2);
        assert_eq!(cache.format_file_info(keeper).unwrap().len(), 2, "the merge is not undone");
    }

    #[test]
    fn the_plan_fingerprint_changes_with_the_plan_the_action_and_the_library() {
        let p = plan(1, &[2, 3]);
        let base = plan_fingerprint(ResolveAction::KeepBestTrashRest, &p, "w1");
        assert_eq!(base, plan_fingerprint(ResolveAction::KeepBestTrashRest, &p, "w1"));
        assert_ne!(base, plan_fingerprint(ResolveAction::MergeAsFormats, &p, "w1"));
        assert_ne!(base, plan_fingerprint(ResolveAction::KeepBestTrashRest, &p, "w2"));
        assert_ne!(base, plan_fingerprint(ResolveAction::KeepBestTrashRest, &plan(2, &[1, 3]), "w1"));
    }
}
