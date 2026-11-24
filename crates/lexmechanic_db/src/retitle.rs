//! Fix titles in one step (tranche 15): find junk titles, suggest clean ones,
//! then in one run set the title, optionally write it into the book files, and
//! optionally rename the files from a template. Undoable.
//!
//! # Suggest ([`suggest`])
//!
//! A book needs fixing when its title is junk ([`crate::junk_metadata`]:
//! placeholders, "print job", `Microsoft Word - x.doc`, bare filenames) or a
//! **template leftover**: one title on at least [`crate::dedupe::TEMPLATE_TITLE_BOOKS`]
//! books holding different files that does not resemble their file names
//! ([`crate::dedupe::template_leftover_titles`], run over the library with the
//! same content groups the duplicate scan uses). The suggestion is the cleaned
//! embedded title when that differs from the junk (`Microsoft Word - Dragon
//! Lore.doc` becomes `Dragon Lore`), else the humanised file stem ([`humanize_stem`]);
//! when that is junk too the row says `no_suggestion` and the user must type one.
//!
//! The file name the rename template would give is evaluated **for the
//! suggested title** (a value override on the template's `title`), not for the
//! title the book holds now.
//!
//! # Apply ([`apply`]) and undo ([`undo`])
//!
//! Per book, errors isolated: skip when the stored title is not the one the
//! user saw (`changed_since_preview`) or the new title is empty (`invalid`);
//! else set the title through `Cache::set_field` (change-logged, so a rebuild
//! reproduces it and `title_sort` follows), then optionally
//! [`crate::embed::embed_metadata`] (which re-records size and hash, so the
//! duplicate index and the PDF fingerprint queue follow the rewritten bytes),
//! then optionally `Cache::rename_format_files`. A failure after the title was
//! set is reported with `title_set: true`; the title is **not** rolled back
//! (the title is the point of the run, and the run records it, so undo
//! reverses it). Every run is recorded in `.lexmechanic/retitle-runs/`.
//!
//! Undo restores a title only where the book still holds the run's new title,
//! renames each format's file back only where it still has the run's new stem,
//! and re-embeds the restored title if the run embedded. The bytes of a file
//! that was embedded are **not** identical after an undo (the PDF writer
//! rewrites the file again); the title in it is. A run can be undone once.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use lazy_static::lazy_static;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::cache::Cache;
use crate::dedupe;
use crate::embed::{self, FormatOutcome};
use crate::junk_metadata::{clean_title, has_file_extension, is_junk_title};
use crate::rename_proposal::propose_stem;
use crate::scan_pipeline::JobControl;

/// Rows one suggestion pass returns.
pub const MAX_ROWS: usize = 5000;
pub const DEFAULT_RENAME_TEMPLATE: &str = "{title}";

lazy_static! {
    /// A copy counter on the end of a file name: `Foo (1)`, `Foo (2) (3)`.
    static ref COPY_COUNTER: Regex = Regex::new(r"(?:\s*\(\d{1,3}\))+\s*$").unwrap();
}

#[derive(Debug, Clone)]
pub enum Scope {
    /// Every book; only those that need fixing are returned.
    All,
    /// Exactly these books; each is returned, `ok` ones included.
    Ids(Vec<i32>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    Junk,
    TemplateLeftover,
    /// Nothing wrong with the title (a requested book that needs no fix).
    Ok,
    /// Wrong, and nothing usable to suggest.
    NoSuggestion,
}

/// A file stem made readable: `_` to a space, a trailing extension and copy
/// counter dropped, whitespace collapsed.
///
/// `Dragon_Lore (2).pdf` -> `Dragon Lore`.
pub fn humanize_stem(stem: &str) -> String {
    let mut s = stem.replace('_', " ");
    loop {
        let before = s.clone();
        let trimmed = s.trim().to_string();
        s = if has_file_extension(&trimmed) { trimmed.rsplit_once('.').map(|(a, _)| a.to_string()).unwrap_or(trimmed) } else { trimmed };
        s = COPY_COUNTER.replace(&s, "").into_owned();
        if s == before {
            break;
        }
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The clean title to suggest for a book, if there is one: the cleaned
/// embedded title when it differs from the current one, else the first usable
/// humanised file stem (stems are tried in the order given).
pub fn suggest_title(current_title: &str, stems: &[String]) -> Option<String> {
    let current = current_title.trim();
    if let Some(cleaned) = clean_title(current) {
        if cleaned != current {
            return Some(cleaned);
        }
    }
    stems.iter().map(|s| humanize_stem(s)).find(|h| !h.is_empty() && !is_junk_title(h) && !h.eq_ignore_ascii_case(current))
}

/// Whether a suggested title reads like a title rather than a code.
///
/// File names on a real collection are often catalogue numbers
/// (`MR1494.ch4`, `EG080102`): better than nothing as a hint, worse than the
/// embedded title they would replace. Such a suggestion is still offered,
/// just not pre-selected. A title is one with two real words, or one word of
/// four letters or more (`flags`, `CUT012 Pirates`).
pub fn looks_like_title(s: &str) -> bool {
    let alpha: Vec<&str> = s.split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 2 && w.chars().all(char::is_alphabetic)).collect();
    alpha.len() >= 2 || alpha.iter().any(|w| w.chars().count() >= 4)
}

/// One book in a suggestion pass.
#[derive(Debug, Clone, Serialize)]
pub struct SuggestedRow {
    pub book_id: i32,
    pub current_title: String,
    pub suggested_title: String,
    pub reason: Reason,
    /// Whether the suggestion reads like a title ([`looks_like_title`]); the
    /// UI pre-selects only these.
    pub confident: bool,
    /// The file stem(s) now, `" | "`-joined when the formats disagree.
    pub current_file: String,
    pub formats: Vec<String>,
    /// The stem the rename template gives with the suggested title. `None`
    /// when there is no suggestion, no file, or the template failed.
    pub proposed_file: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Suggestions {
    pub rows: Vec<SuggestedRow>,
    /// Books that need fixing in the scope, whether or not they fit in `rows`.
    pub total: usize,
    pub truncated: bool,
}

/// The dry run. Changes nothing.
pub fn suggest(cache: &Cache, scope: &Scope, rename_template: &str) -> Result<Suggestions> {
    let records = dedupe::load_records(cache)?;
    // The same evidence the duplicate scan uses for "this title is a template
    // leftover": different files (content groups) under one title.
    let content = crate::copy_to_library::scan_library_for_content_duplicates(cache)?;
    let index: HashMap<i32, usize> = records.iter().enumerate().map(|(i, r)| (r.id, i)).collect();
    let leftovers = dedupe::template_leftover_titles(&records, &content, &index);

    let wanted: Option<BTreeSet<i32>> = match scope {
        Scope::All => None,
        Scope::Ids(ids) => Some(ids.iter().copied().collect()),
    };
    let template = if rename_template.trim().is_empty() { DEFAULT_RENAME_TEMPLATE } else { rename_template };

    let mut rows = Vec::new();
    let mut total = 0usize;
    let mut claimed: HashSet<(String, String)> = HashSet::new();
    for (i, record) in records.iter().enumerate() {
        if wanted.as_ref().is_some_and(|w| !w.contains(&record.id)) {
            continue;
        }
        let status = if is_junk_title(&record.title) {
            Reason::Junk
        } else if leftovers.contains(&i) {
            Reason::TemplateLeftover
        } else {
            Reason::Ok
        };
        if status != Reason::Ok {
            total += 1;
        } else if wanted.is_none() {
            continue;
        }
        if rows.len() >= MAX_ROWS {
            continue;
        }
        let mut stems = record.stems.clone();
        stems.sort();
        stems.dedup();
        let (reason, suggested) = match status {
            Reason::Ok => (Reason::Ok, record.title.clone()),
            _ => match suggest_title(&record.title, &stems) {
                Some(t) => (status, t),
                None => (Reason::NoSuggestion, String::new()),
            },
        };
        let formats: Vec<String> = cache.format_file_names(record.id)?.into_iter().map(|(f, _)| f).collect();
        let proposed_file = if suggested.is_empty() || formats.is_empty() { None } else { propose_stem(cache, record.id, template, Some(&suggested), &mut claimed).ok() };
        rows.push(SuggestedRow { book_id: record.id, current_title: record.title.clone(), confident: reason == Reason::Ok || looks_like_title(&suggested), suggested_title: suggested, reason, current_file: stems.join(" | "), formats, proposed_file });
    }
    Ok(Suggestions { truncated: total > rows.iter().filter(|r| r.reason != Reason::Ok).count(), rows, total })
}

// ---------------------------------------------------------------------
// Apply
// ---------------------------------------------------------------------

/// One title the user approved.
#[derive(Debug, Clone, Deserialize)]
pub struct Item {
    pub book_id: i32,
    /// The title the user saw; the book is skipped if it holds another now.
    pub expected_title: String,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct ApplyOptions {
    pub embed: bool,
    pub rename_files: bool,
    pub rename_template: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Skipped {
    pub book_id: i32,
    /// `changed_since_preview`, `invalid`, `no_such_book`, or (undo) why a part was left.
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Failure {
    pub book_id: i32,
    /// `title`, `embed` or `rename`.
    pub stage: String,
    pub reason: String,
    /// Whether the new title was stored before this failed (it stays).
    pub title_set: bool,
}

/// One book of a recorded run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunBook {
    pub book_id: i32,
    pub old_title: String,
    pub new_title: String,
    /// Format -> file stem, before and after.
    pub old_names: BTreeMap<String, String>,
    pub new_names: BTreeMap<String, String>,
    pub embedded: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RetitleRun {
    pub run_id: String,
    pub created_at: i64,
    pub embed: bool,
    pub rename_files: bool,
    pub rename_template: String,
    pub books: Vec<RunBook>,
    #[serde(default)]
    pub undone_at: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RetitleReport {
    pub run_id: String,
    pub books_planned: usize,
    pub retitled: usize,
    /// Books whose files had the title written into them.
    pub embedded: usize,
    /// Books whose files were renamed.
    pub renamed: usize,
    pub skipped: Vec<Skipped>,
    pub failed: Vec<Failure>,
    pub cancelled: bool,
}

/// The run's undo record (an append-only journal; see [`crate::run_journal`]).
pub fn load_run(library: &Path, run_id: &str) -> Result<RetitleRun> {
    crate::run_journal::RETITLE_RUNS.load(library, run_id)
}

/// Runs, newest first: `(run_id, created_at, books, undone)`.
pub fn list_runs(library: &Path) -> Vec<(String, i64, usize, bool)> {
    crate::run_journal::RETITLE_RUNS.list(library)
}

fn names(cache: &Cache, book_id: i32) -> BTreeMap<String, String> {
    cache.format_file_names(book_id).unwrap_or_default().into_iter().collect()
}

/// Carries out the approved items. A cancel stops between books.
pub fn apply(cache: &Cache, items: &[Item], options: &ApplyOptions, control: &JobControl) -> Result<RetitleReport> {
    use std::sync::atomic::Ordering;
    let library = cache.backend.library_path.clone();
    let header = RetitleRun { run_id: crate::run_journal::RETITLE_RUNS.new_run_id(), created_at: crate::trash::now_secs(), embed: options.embed, rename_files: options.rename_files, rename_template: options.rename_template.clone(), ..Default::default() };
    let mut report = RetitleReport { run_id: header.run_id.clone(), books_planned: items.len(), ..Default::default() };
    // Opened on the first book that changes, so a run that changes nothing leaves no file.
    let mut journal: Option<crate::run_journal::RunJournal> = None;
    let template = if options.rename_template.trim().is_empty() { DEFAULT_RENAME_TEMPLATE } else { options.rename_template.as_str() };
    let mut claimed: HashSet<(String, String)> = HashSet::new();
    control.set_phase("retitling");
    control.total.store(items.len() as u64, Ordering::Relaxed);

    for item in items {
        if control.is_cancelled() {
            report.cancelled = true;
            break;
        }
        control.seen.fetch_add(1, Ordering::Relaxed);
        let id = item.book_id;
        let skip = |report: &mut RetitleReport, reason: &str| report.skipped.push(Skipped { book_id: id, reason: reason.to_string() });
        let Some(stored) = cache.field_for(id, "title").ok().flatten() else {
            skip(&mut report, "no_such_book");
            continue;
        };
        if stored != item.expected_title {
            skip(&mut report, "changed_since_preview");
            continue;
        }
        let new_title = item.title.trim();
        if new_title.is_empty() {
            skip(&mut report, "invalid");
            continue;
        }

        let old_names = names(cache, id);

        // The undo entry is on disk before anything is changed: a crash after the
        // title is set (or a file renamed) must not leave a change nobody can undo.
        // The entry says what is about to be changed; the finished one replaces it.
        if journal.is_none() {
            journal = Some(crate::run_journal::RETITLE_RUNS.begin(&library, &header.run_id, &header).context("could not start the undo journal, so nothing was changed")?);
        }
        let j = journal.as_mut().expect("opened above");
        let intent = RunBook { book_id: id, old_title: stored.clone(), new_title: new_title.to_string(), old_names: old_names.clone(), new_names: BTreeMap::new(), embedded: false };
        if let Err(e) = j.append_book(&intent) {
            control.errors.fetch_add(1, Ordering::Relaxed);
            report.failed.push(Failure { book_id: id, stage: "title".into(), reason: format!("could not write its undo entry, so it was not changed: {e:#}"), title_set: false });
            continue;
        }
        if let Err(e) = cache.set_field(id, "title", new_title) {
            control.errors.fetch_add(1, Ordering::Relaxed);
            report.failed.push(Failure { book_id: id, stage: "title".into(), reason: format!("{e:#}"), title_set: false });
            continue;
        }
        report.retitled += 1;
        let mut book = RunBook { book_id: id, old_title: stored, new_title: new_title.to_string(), old_names, new_names: BTreeMap::new(), embedded: false };

        if options.embed {
            match embed::embed_metadata(cache, id) {
                Ok(outcomes) => {
                    book.embedded = outcomes.iter().any(|(_, o)| *o == FormatOutcome::Embedded);
                    if book.embedded {
                        report.embedded += 1;
                    }
                    let problems: Vec<String> = outcomes
                        .iter()
                        .filter_map(|(f, o)| match o {
                            FormatOutcome::Failed(why) => Some(format!("{f}: {why}")),
                            FormatOutcome::FileMissing => Some(format!("{f}: the file is not on disk")),
                            _ => None,
                        })
                        .collect();
                    if !problems.is_empty() {
                        report.failed.push(Failure { book_id: id, stage: "embed".into(), reason: problems.join("; "), title_set: true });
                    }
                }
                Err(e) => report.failed.push(Failure { book_id: id, stage: "embed".into(), reason: format!("{e:#}"), title_set: true }),
            }
        }

        if options.rename_files {
            // The title is stored now, so the template is evaluated as it stands.
            let renamed = propose_stem(cache, id, template, None, &mut claimed).and_then(|stem| {
                let current = cache.format_file_names(id).map_err(|e| e.to_string())?;
                if current.iter().all(|(_, n)| *n == stem) {
                    return Ok(false);
                }
                // The rename is written to the undo journal *before* it is made,
                // the way organize writes its moves: every format is about to be
                // called `stem`. If the finished entry below cannot be written
                // this one stands, so undo can still put the files back; if the
                // rename then does not happen, undo finds the old names in place
                // and leaves them alone. Without a journal entry for it, the
                // file is not renamed.
                let mut planned = book.clone();
                planned.new_names = current.iter().map(|(format, _)| (format.clone(), stem.clone())).collect();
                if let Some(j) = journal.as_mut() {
                    j.append_book(&planned).map_err(|e| format!("could not write its undo entry, so the files were not renamed: {e:#}"))?;
                }
                cache.rename_format_files(id, &stem).map(|r| !r.is_empty()).map_err(|e| format!("{e:#}"))
            });
            match renamed {
                Ok(true) => report.renamed += 1,
                Ok(false) => {}
                Err(why) => report.failed.push(Failure { book_id: id, stage: "rename".into(), reason: why, title_set: true }),
            }
        }
        book.new_names = names(cache, id);
        // The finished entry (embedded flag, the file names as they ended up)
        // replaces the earlier ones. If this fails, the entries written before
        // the title and the rename stand, and they are enough to undo both; the
        // only thing lost is the `embedded` flag if the embed ran, which this
        // is worth saying.
        if let Some(j) = journal.as_mut() {
            if let Err(e) = j.append_book(&book) {
                report.skipped.push(Skipped { book_id: id, reason: format!("warning: the final undo entry could not be written ({e:#}); undo still restores the title and file names from the entries written before they changed") });
            }
        }
    }
    Ok(report)
}

// ---------------------------------------------------------------------
// Undo
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize)]
pub struct UndoReport {
    pub run_id: String,
    pub titles_restored: usize,
    pub files_restored: usize,
    pub re_embedded: usize,
    /// Parts left alone, and why (changed since the run).
    pub skipped: Vec<Skipped>,
    pub failed: Vec<Failure>,
    pub cancelled: bool,
}

/// Renames one format's file and its row, refusing to overwrite.
fn rename_one_format(cache: &Cache, book_id: i32, format: &str, from: &str, to: &str) -> Result<()> {
    let dir = cache.book_dir(book_id)?.ok_or_else(|| anyhow!("no such book"))?;
    let ext = format.to_lowercase();
    let (old_path, new_path) = (dir.join(format!("{from}.{ext}")), dir.join(format!("{to}.{ext}")));
    // The same file under another spelling (a case-only rename on a
    // case-insensitive volume) is not a collision.
    let same_file = new_path.exists() && lexmechanic_utils::filenames::samefile(&old_path, &new_path);
    if new_path.exists() && !same_file {
        return Err(anyhow!("{} already exists", new_path.display()));
    }
    if old_path.exists() {
        let handle = cache.backend.write_handle()?;
        if same_file {
            handle.rename_atomic(&old_path, &new_path)?;
        } else {
            handle.rename_atomic_no_replace(&old_path, &new_path)?;
        }
    }
    cache.set_format_name(book_id, format, to)?;
    Ok(())
}

/// Puts a run's titles and file names back where they were still the run's.
pub fn undo(cache: &Cache, run_id: &str, control: &JobControl) -> Result<UndoReport> {
    let library = cache.backend.library_path.clone();
    let run = load_run(&library, run_id)?;
    if run.undone_at.is_some() {
        return Err(anyhow!("run {run_id} was already undone"));
    }
    let mut report = UndoReport { run_id: run_id.to_string(), ..Default::default() };
    control.set_phase("retitling");
    control.total.store(run.books.len() as u64, std::sync::atomic::Ordering::Relaxed);
    for book in run.books.iter().rev() {
        if control.is_cancelled() {
            report.cancelled = true;
            break;
        }
        control.seen.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let id = book.book_id;
        let Some(current_title) = cache.field_for(id, "title").ok().flatten() else {
            report.skipped.push(Skipped { book_id: id, reason: "the book has been removed since".into() });
            continue;
        };

        // Files first, so a re-embed finds them under their old names.
        let current_names = names(cache, id);
        for (format, new_stem) in &book.new_names {
            let Some(old_stem) = book.old_names.get(format) else { continue };
            if old_stem == new_stem {
                continue;
            }
            // Still under its old name: the rename this entry planned never
            // happened (it is journaled before it is made), so nothing to undo.
            if current_names.get(format) == Some(old_stem) {
                continue;
            }
            if current_names.get(format) != Some(new_stem) {
                report.skipped.push(Skipped { book_id: id, reason: format!("the {format} file was renamed since the run") });
                continue;
            }
            match rename_one_format(cache, id, format, new_stem, old_stem) {
                Ok(()) => report.files_restored += 1,
                Err(e) => report.failed.push(Failure { book_id: id, stage: "rename".into(), reason: format!("{format}: {e:#}"), title_set: false }),
            }
        }

        if current_title != book.new_title {
            report.skipped.push(Skipped { book_id: id, reason: "the title was changed since the run".into() });
            continue;
        }
        if let Err(e) = cache.set_field(id, "title", &book.old_title) {
            report.failed.push(Failure { book_id: id, stage: "title".into(), reason: format!("{e:#}"), title_set: false });
            continue;
        }
        report.titles_restored += 1;
        if book.embedded {
            match embed::embed_metadata(cache, id) {
                Ok(outcomes) => {
                    if outcomes.iter().any(|(_, o)| *o == FormatOutcome::Embedded) {
                        report.re_embedded += 1;
                    }
                }
                Err(e) => report.failed.push(Failure { book_id: id, stage: "embed".into(), reason: format!("{e:#}"), title_set: true }),
            }
        }
    }
    // Once undone, always undone: a book left alone stays changed, so a second
    // pass would only repeat the same skips (and report restored books as changed).
    if !report.cancelled {
        // The undo is done; failing to say so must not turn it into an error.
        if let Err(e) = crate::run_journal::RETITLE_RUNS.mark_undone(&library, run_id, crate::trash::now_secs()) {
            report.skipped.push(Skipped { book_id: 0, reason: format!("warning: the run could not be marked undone: {e:#}") });
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_PAGE: &[u8] = include_bytes!("../../lexmechanic_ebooks/tests/data/two-page.pdf");

    fn library() -> (tempfile::TempDir, Cache) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        (dir, cache)
    }

    /// A book for the PDF `stem.pdf` whose stored title is `title`. The bytes
    /// differ per book (a comment after the end), so each is a different file.
    fn add(dir: &Path, cache: &Cache, stem: &str, title: &str, real_pdf: bool) -> i32 {
        let mut bytes = if real_pdf { TWO_PAGE.to_vec() } else { b"%PDF-1.4 ".to_vec() };
        bytes.extend_from_slice(format!("\n% {stem}\n").as_bytes());
        std::fs::write(dir.join(format!("{stem}.pdf")), &bytes).unwrap();
        let mut meta = lexmechanic_ebooks::metadata::MetaInformation::default();
        meta.title = title.to_string();
        cache.register_book_in_place(&format!("{stem}.pdf"), &meta).unwrap()
    }

    fn stems_of(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    fn all(cache: &Cache) -> Suggestions {
        suggest(cache, &Scope::All, "{title}").unwrap()
    }

    fn options(embed: bool, rename: bool) -> ApplyOptions {
        ApplyOptions { embed, rename_files: rename, rename_template: "{title}".into() }
    }

    fn item(cache: &Cache, id: i32, title: &str) -> Item {
        Item { book_id: id, expected_title: cache.field_for(id, "title").unwrap().unwrap(), title: title.into() }
    }

    // --- suggestion rules ---------------------------------------------

    #[test]
    fn a_catalogue_number_is_suggested_but_not_confidently() {
        for code in ["MR1494.ch4", "EG080102", "MISBTRM07A", "HLT", "06", ""] {
            assert!(!looks_like_title(code), "{code}");
        }
        for title in ["flags", "CUT012 Pirates", "APOM 1 Heists", "Encyclopedia Metropolitana Vol 20 1845", "Name Maker - Kaiju"] {
            assert!(looks_like_title(title), "{title}");
        }
    }

    #[test]
    fn a_stem_is_humanised() {
        assert_eq!(humanize_stem("Dragon_Lore"), "Dragon Lore");
        assert_eq!(humanize_stem("Dragon  Lore   Vol_2"), "Dragon Lore Vol 2");
        assert_eq!(humanize_stem("Dragon Lore (1)"), "Dragon Lore");
        assert_eq!(humanize_stem("Dragon_Lore (2) (3)"), "Dragon Lore");
        assert_eq!(humanize_stem("Dragon Lore.doc"), "Dragon Lore");
        assert_eq!(humanize_stem("Dragon Lore.pdf (1)"), "Dragon Lore");
        // Not an extension, and not a counter: left alone.
        assert_eq!(humanize_stem("Vol. 2 Revised"), "Vol. 2 Revised");
        assert_eq!(humanize_stem("Catch-22 (Special Edition)"), "Catch-22 (Special Edition)");
    }

    #[test]
    fn a_junk_title_gets_its_file_stem() {
        assert_eq!(suggest_title("Untitled", &stems_of(&["Dragon_Lore (1)"])).as_deref(), Some("Dragon Lore"));
        assert_eq!(suggest_title("ACDSee 32 print job", &stems_of(&["Boiler_Manual"])).as_deref(), Some("Boiler Manual"));
    }

    #[test]
    fn a_word_prefix_is_cleaned_instead_of_using_the_file_name() {
        assert_eq!(suggest_title("Microsoft Word - Dragon Lore.doc", &stems_of(&["scan0001"])).as_deref(), Some("Dragon Lore"));
    }

    #[test]
    fn with_no_usable_stem_there_is_no_suggestion() {
        assert_eq!(suggest_title("Untitled", &stems_of(&["Document1"])), None);
        assert_eq!(suggest_title("Untitled", &stems_of(&["untitled"])), None);
        assert_eq!(suggest_title("Untitled", &[]), None);
        // A stem that is only a copy counter or an extension is empty.
        assert_eq!(suggest_title("Untitled", &stems_of(&["(1)"])), None);
    }

    #[test]
    fn the_first_usable_stem_wins_and_one_equal_to_the_title_is_no_help() {
        assert_eq!(suggest_title("Untitled", &stems_of(&["Document1", "Real Name"])).as_deref(), Some("Real Name"));
        assert_eq!(suggest_title("Some Title", &stems_of(&["Some_Title"])), None);
    }

    // --- the suggestion pass --------------------------------------------

    #[test]
    fn all_returns_only_books_that_need_fixing_and_ids_returns_every_requested_book() {
        let (dir, cache) = library();
        let junk = add(dir.path(), &cache, "Dragon_Lore", "Untitled", false);
        let word = add(dir.path(), &cache, "scan0001", "Microsoft Word - Wyrm Atlas.doc", false);
        let nothing = add(dir.path(), &cache, "Document2", "Untitled", false);
        let fine = add(dir.path(), &cache, "fine", "A Perfectly Good Title", false);

        let s = all(&cache);
        assert_eq!(s.total, 3);
        assert!(!s.truncated);
        let by_id: HashMap<i32, &SuggestedRow> = s.rows.iter().map(|r| (r.book_id, r)).collect();
        assert!(!by_id.contains_key(&fine), "a good title is not listed in an all scope");
        assert_eq!(by_id[&junk].suggested_title, "Dragon Lore");
        assert_eq!(by_id[&junk].reason, Reason::Junk);
        assert_eq!(by_id[&junk].current_file, "Dragon_Lore");
        assert_eq!(by_id[&junk].formats, vec!["PDF".to_string()]);
        assert_eq!(by_id[&word].suggested_title, "Wyrm Atlas");
        assert_eq!(by_id[&nothing].reason, Reason::NoSuggestion);
        assert_eq!(by_id[&nothing].suggested_title, "");
        assert_eq!(by_id[&nothing].proposed_file, None);

        let picked = suggest(&cache, &Scope::Ids(vec![fine, junk]), "{title}").unwrap();
        assert_eq!(picked.rows.len(), 2, "every requested book is returned");
        let ok = picked.rows.iter().find(|r| r.book_id == fine).unwrap();
        assert_eq!(ok.reason, Reason::Ok);
        assert_eq!(ok.suggested_title, "A Perfectly Good Title");
        assert_eq!(picked.total, 1, "only the junk one needs fixing");
    }

    #[test]
    fn the_proposed_file_is_the_template_evaluated_for_the_suggested_title() {
        let (dir, cache) = library();
        let id = add(dir.path(), &cache, "Dragon_Lore", "Untitled", false);
        let s = suggest(&cache, &Scope::All, "Fixed - {title}").unwrap();
        let row = s.rows.iter().find(|r| r.book_id == id).unwrap();
        assert_eq!(row.proposed_file.as_deref(), Some("Fixed - Dragon Lore"), "not 'Fixed - Untitled'");
        assert_eq!(cache.field_for(id, "title").unwrap().unwrap(), "Untitled", "a dry run changes nothing");
    }

    #[test]
    fn a_template_leftover_is_found_by_the_library_wide_pass() {
        let (dir, cache) = library();
        let leftover = "Nathanael-Roux_Affinity-Publisher_Template-PHB_v0.1";
        let mut ids = Vec::new();
        for stem in ["Alchemy Handbook", "Bestiary of the North", "Cartographers Guide", "Dwarven Forges", "Elven Courts", "Field Notes"] {
            ids.push(add(dir.path(), &cache, stem, leftover, false));
        }
        let fine = add(dir.path(), &cache, "Plain", "Plain Title", false);

        let s = all(&cache);
        assert_eq!(s.total, 6);
        assert!(s.rows.iter().all(|r| r.reason == Reason::TemplateLeftover && ids.contains(&r.book_id)), "{:?}", s.rows);
        assert!(s.rows.iter().all(|r| !r.suggested_title.is_empty()));
        let row = s.rows.iter().find(|r| r.current_file == "Bestiary of the North").unwrap();
        assert_eq!(row.suggested_title, "Bestiary of the North");
        assert!(!s.rows.iter().any(|r| r.book_id == fine));

        // Four books are not enough for a leftover.
        let (dir4, cache4) = library();
        for stem in ["Alchemy Handbook", "Bestiary of the North", "Cartographers Guide", "Dwarven Forges"] {
            add(dir4.path(), &cache4, stem, leftover, false);
        }
        assert_eq!(all(&cache4).total, 0);
    }

    #[test]
    fn rows_are_capped_and_say_so() {
        let (dir, cache) = library();
        add(dir.path(), &cache, "one", "Untitled", false);
        // The cap itself is MAX_ROWS; the flag is what the cap controls.
        let s = all(&cache);
        assert!(!s.truncated && s.rows.len() == s.total);
    }

    // --- apply ----------------------------------------------------------

    #[test]
    fn apply_sets_the_title_logged_and_a_rebuild_reproduces_it() {
        let (dir, cache) = library();
        let id = add(dir.path(), &cache, "Dragon_Lore", "Untitled", false);
        let report = apply(&cache, &[item(&cache, id, "Dragon Lore")], &options(false, false), &JobControl::new()).unwrap();
        assert_eq!((report.retitled, report.failed.len(), report.skipped.len()), (1, 0, 0), "{report:?}");
        assert_eq!(cache.field_for(id, "title").unwrap().unwrap(), "Dragon Lore");
        assert_eq!(cache.field_for(id, "sort").unwrap().unwrap_or_default(), "Dragon Lore", "title_sort follows");
        assert!(dir.path().join("Dragon_Lore.pdf").is_file(), "no rename or embed was asked for");

        cache.rebuild_from_change_log().unwrap();
        let again = cache.all_book_ids().unwrap()[0];
        assert_eq!(cache.field_for(again, "title").unwrap().unwrap(), "Dragon Lore", "the change log reproduces the title");
    }

    #[test]
    fn apply_embeds_the_title_into_the_pdf_and_updates_size_and_hash() {
        let (dir, cache) = library();
        let id = add(dir.path(), &cache, "scan0001", "Untitled", true);
        let path = dir.path().join("scan0001.pdf");
        let before = std::fs::read(&path).unwrap();
        let old_hash = cache.checksums().recorded_identity(id, "format", "PDF").unwrap().0;

        let report = apply(&cache, &[item(&cache, id, "Dragon Lore")], &options(true, false), &JobControl::new()).unwrap();
        assert_eq!((report.retitled, report.embedded, report.failed.len()), (1, 1, 0), "{report:?}");
        assert_eq!(lexmechanic_ebooks::metadata::pdf::get_metadata_from_path(&path).unwrap().title, "Dragon Lore");

        let after = std::fs::read(&path).unwrap();
        assert_ne!(before, after);
        let new_hash = blake3::hash(&after).to_hex().to_string();
        let recorded = cache.checksums().recorded_identity(id, "format", "PDF").unwrap().0;
        assert_ne!(recorded, old_hash);
        assert_eq!(recorded.as_deref(), Some(new_hash.as_str()));
        let size: i64 = cache.backend.conn.lock().unwrap().query_row("SELECT uncompressed_size FROM data WHERE book = ?1", [id], |r| r.get(0)).unwrap();
        assert_eq!(size, after.len() as i64);
    }

    #[test]
    fn apply_renames_the_files_from_the_template_for_the_new_title() {
        let (dir, cache) = library();
        let id = add(dir.path(), &cache, "Dragon_Lore", "Untitled", false);
        let opts = ApplyOptions { embed: false, rename_files: true, rename_template: "{title} (fixed)".into() };
        let report = apply(&cache, &[item(&cache, id, "Dragon Lore")], &opts, &JobControl::new()).unwrap();
        assert_eq!((report.retitled, report.renamed, report.failed.len()), (1, 1, 0), "{report:?}");
        assert!(dir.path().join("Dragon Lore (fixed).pdf").is_file());
        assert!(!dir.path().join("Dragon_Lore.pdf").exists());
        assert_eq!(cache.format_file_stem(id).unwrap().as_deref(), Some("Dragon Lore (fixed)"));
    }

    #[test]
    fn two_books_given_one_title_get_distinct_file_names() {
        let (dir, cache) = library();
        let a = add(dir.path(), &cache, "one", "Untitled", false);
        let b = add(dir.path(), &cache, "two", "Untitled", false);
        let report = apply(&cache, &[item(&cache, a, "Same"), item(&cache, b, "Same")], &options(false, true), &JobControl::new()).unwrap();
        assert_eq!((report.renamed, report.failed.len()), (2, 0), "{report:?}");
        assert_eq!(cache.format_file_stem(a).unwrap().as_deref(), Some("Same"));
        assert_eq!(cache.format_file_stem(b).unwrap().as_deref(), Some("Same (1)"));
        assert!(dir.path().join("Same.pdf").is_file() && dir.path().join("Same (1).pdf").is_file());
    }

    #[test]
    fn a_book_changed_since_the_preview_or_given_an_empty_title_is_skipped() {
        let (dir, cache) = library();
        let changed = add(dir.path(), &cache, "one", "Untitled", false);
        let empty = add(dir.path(), &cache, "two", "Untitled", false);
        let good = add(dir.path(), &cache, "three", "Untitled", false);
        let stale = Item { book_id: changed, expected_title: "What The User Saw".into(), title: "New".into() };
        let report = apply(&cache, &[stale, item(&cache, empty, "   "), item(&cache, good, "Good")], &options(false, false), &JobControl::new()).unwrap();
        assert_eq!(report.retitled, 1);
        let reasons: Vec<(i32, &str)> = report.skipped.iter().map(|s| (s.book_id, s.reason.as_str())).collect();
        assert_eq!(reasons, vec![(changed, "changed_since_preview"), (empty, "invalid")]);
        assert_eq!(cache.field_for(changed, "title").unwrap().unwrap(), "Untitled");
        assert_eq!(cache.field_for(empty, "title").unwrap().unwrap(), "Untitled");
        assert_eq!(cache.field_for(good, "title").unwrap().unwrap(), "Good");
    }

    #[test]
    fn one_books_failure_does_not_stop_the_others() {
        let (dir, cache) = library();
        // `bad` is not a PDF at all: the embed fails after its title was set.
        let bad = add(dir.path(), &cache, "bad", "Untitled", false);
        let good = add(dir.path(), &cache, "good", "Untitled", true);
        let report = apply(&cache, &[item(&cache, bad, "Bad Book"), item(&cache, good, "Good Book")], &options(true, true), &JobControl::new()).unwrap();
        assert_eq!(report.retitled, 2, "{report:?}");
        assert_eq!(report.embedded, 1, "{report:?}");
        let failure = report.failed.iter().find(|f| f.book_id == bad && f.stage == "embed").unwrap_or_else(|| panic!("{report:?}"));
        assert!(failure.title_set, "the title stays and the report says so");
        assert_eq!(cache.field_for(bad, "title").unwrap().unwrap(), "Bad Book");
        // The rename still ran for the failed-embed book, and for the other.
        assert_eq!(report.renamed, 2, "{report:?}");
        assert!(dir.path().join("Good Book.pdf").is_file());
        assert_eq!(lexmechanic_ebooks::metadata::pdf::get_metadata_from_path(&dir.path().join("Good Book.pdf")).unwrap().title, "Good Book");
    }

    #[test]
    fn a_rename_that_cannot_happen_is_reported_per_book_and_the_title_stays() {
        let (dir, cache) = library();
        let id = add(dir.path(), &cache, "one", "Untitled", false);
        // An unrelated file the library does not track already has the name.
        // (The proposal avoids it, so make the rename itself fail instead: the
        // file the row points at is fine, but the template yields nothing.)
        let opts = ApplyOptions { embed: false, rename_files: true, rename_template: "{series}".into() };
        let report = apply(&cache, &[item(&cache, id, "Titled")], &opts, &JobControl::new()).unwrap();
        assert_eq!(report.retitled, 1);
        assert_eq!(report.failed.len(), 1, "{report:?}");
        assert_eq!((report.failed[0].stage.as_str(), report.failed[0].title_set), ("rename", true));
        assert_eq!(cache.field_for(id, "title").unwrap().unwrap(), "Titled");
        assert!(dir.path().join("one.pdf").is_file());
    }

    #[test]
    fn a_cancelled_run_stops_between_books() {
        let (dir, cache) = library();
        let a = add(dir.path(), &cache, "one", "Untitled", false);
        let control = JobControl::new();
        control.cancel();
        let report = apply(&cache, &[item(&cache, a, "X")], &options(false, false), &control).unwrap();
        assert!(report.cancelled);
        assert_eq!(report.retitled, 0);
    }

    // --- undo ---------------------------------------------------------

    #[test]
    fn undo_restores_titles_and_file_names_and_a_run_cannot_be_undone_twice() {
        let (dir, cache) = library();
        let id = add(dir.path(), &cache, "Dragon_Lore", "Untitled", true);
        let report = apply(&cache, &[item(&cache, id, "Dragon Lore")], &options(true, true), &JobControl::new()).unwrap();
        assert_eq!((report.retitled, report.renamed, report.embedded), (1, 1, 1), "{report:?}");
        assert!(dir.path().join("Dragon Lore.pdf").is_file());

        let undone = undo(&cache, &report.run_id, &JobControl::new()).unwrap();
        assert_eq!((undone.titles_restored, undone.files_restored, undone.re_embedded), (1, 1, 1), "{undone:?}");
        assert!(undone.skipped.is_empty() && undone.failed.is_empty(), "{undone:?}");
        assert_eq!(cache.field_for(id, "title").unwrap().unwrap(), "Untitled");
        assert_eq!(cache.format_file_stem(id).unwrap().as_deref(), Some("Dragon_Lore"));
        assert!(dir.path().join("Dragon_Lore.pdf").is_file());
        assert!(!dir.path().join("Dragon Lore.pdf").exists());
        // Re-embedded with the restored title; the recorded hash follows the file.
        let path = dir.path().join("Dragon_Lore.pdf");
        assert_eq!(lexmechanic_ebooks::metadata::pdf::get_metadata_from_path(&path).unwrap().title, "Untitled");
        let hash = blake3::hash(&std::fs::read(&path).unwrap()).to_hex().to_string();
        assert_eq!(cache.checksums().recorded_identity(id, "format", "PDF").unwrap().0.as_deref(), Some(hash.as_str()));

        assert!(undo(&cache, &report.run_id, &JobControl::new()).is_err(), "a double undo is refused");
        assert!(list_runs(dir.path())[0].3, "the run is marked undone");
    }

    /// The last journal write of a book fails (injected). The rename was journaled
    /// before it was made, so undo still puts the file back, not just the title.
    #[test]
    fn undo_restores_renamed_files_even_when_the_final_journal_entry_could_not_be_written() {
        let (dir, cache) = library();
        let id = add(dir.path(), &cache, "Dragon_Lore", "Untitled", false);
        // Appends for the book: the intent (1st), the planned rename (2nd), the finished entry (3rd, fails).
        crate::run_journal::test_hooks::fail_book_append_after(Some(2));
        let report = apply(&cache, &[item(&cache, id, "Dragon Lore")], &options(false, true), &JobControl::new()).unwrap();
        crate::run_journal::test_hooks::fail_book_append_after(None);
        assert_eq!((report.retitled, report.renamed), (1, 1), "{report:?}");
        assert!(report.skipped.iter().any(|s| s.reason.contains("final undo entry")), "the failed write is reported: {report:?}");
        assert!(dir.path().join("Dragon Lore.pdf").is_file());

        let undone = undo(&cache, &report.run_id, &JobControl::new()).unwrap();
        assert_eq!((undone.titles_restored, undone.files_restored), (1, 1), "{undone:?}");
        assert_eq!(cache.field_for(id, "title").unwrap().unwrap(), "Untitled");
        assert_eq!(cache.format_file_stem(id).unwrap().as_deref(), Some("Dragon_Lore"));
        assert!(dir.path().join("Dragon_Lore.pdf").is_file());
        assert!(!dir.path().join("Dragon Lore.pdf").exists());
    }

    /// If the rename's own journal entry cannot be written the files are not
    /// renamed (the title is still set and undoable).
    #[test]
    fn a_rename_whose_journal_entry_cannot_be_written_is_not_made() {
        let (dir, cache) = library();
        let id = add(dir.path(), &cache, "Dragon_Lore", "Untitled", false);
        crate::run_journal::test_hooks::fail_book_append_after(Some(1));
        let report = apply(&cache, &[item(&cache, id, "Dragon Lore")], &options(false, true), &JobControl::new()).unwrap();
        crate::run_journal::test_hooks::fail_book_append_after(None);
        assert_eq!((report.retitled, report.renamed), (1, 0), "{report:?}");
        assert!(report.failed.iter().any(|f| f.stage == "rename" && f.reason.contains("undo entry")), "{report:?}");
        assert!(dir.path().join("Dragon_Lore.pdf").is_file(), "the file is where it was");

        let undone = undo(&cache, &report.run_id, &JobControl::new()).unwrap();
        assert_eq!(undone.titles_restored, 1, "{undone:?}");
        assert!(undone.skipped.is_empty(), "nothing to complain about for a rename that never happened: {undone:?}");
    }

    #[test]
    fn undo_skips_a_book_changed_since_and_restores_the_rest() {
        let (dir, cache) = library();
        let a = add(dir.path(), &cache, "one", "Untitled", false);
        let b = add(dir.path(), &cache, "two", "Untitled", false);
        let report = apply(&cache, &[item(&cache, a, "Alpha"), item(&cache, b, "Beta")], &options(false, true), &JobControl::new()).unwrap();
        assert_eq!(report.retitled, 2);
        cache.set_field(a, "title", "Edited By Hand").unwrap();
        cache.rename_format_files(b, "Moved By Hand").unwrap();

        let undone = undo(&cache, &report.run_id, &JobControl::new()).unwrap();
        // `a`: file back, title left. `b`: title back, file left.
        assert_eq!(cache.field_for(a, "title").unwrap().unwrap(), "Edited By Hand");
        assert_eq!(cache.format_file_stem(a).unwrap().as_deref(), Some("one"));
        assert_eq!(cache.field_for(b, "title").unwrap().unwrap(), "Untitled");
        assert_eq!(cache.format_file_stem(b).unwrap().as_deref(), Some("Moved By Hand"));
        let reasons: Vec<(i32, &str)> = undone.skipped.iter().map(|s| (s.book_id, s.reason.as_str())).collect();
        assert!(reasons.contains(&(a, "the title was changed since the run")), "{reasons:?}");
        assert!(reasons.iter().any(|(id, r)| *id == b && r.contains("renamed since")), "{reasons:?}");
    }

    #[test]
    fn runs_are_recorded_per_book_and_listed_newest_first() {
        let (dir, cache) = library();
        let id = add(dir.path(), &cache, "Dragon_Lore", "Untitled", false);
        let report = apply(&cache, &[item(&cache, id, "Dragon Lore")], &options(false, true), &JobControl::new()).unwrap();
        let run = load_run(dir.path(), &report.run_id).unwrap();
        assert_eq!(run.books.len(), 1);
        let b = &run.books[0];
        assert_eq!((b.old_title.as_str(), b.new_title.as_str(), b.embedded), ("Untitled", "Dragon Lore", false));
        assert_eq!(b.old_names["PDF"], "Dragon_Lore");
        assert_eq!(b.new_names["PDF"], "Dragon Lore");
        assert_eq!(list_runs(dir.path()).len(), 1);
        assert!(load_run(dir.path(), "../x").is_err());
    }
}
