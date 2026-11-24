//! The undo record of a bulk run (organize, retitle): an append-only journal.
//!
//! A run used to be one JSON document rewritten every 100 books and once more
//! at the end. A crash left up to 99 moved books with no undo entry, and a
//! failed final write turned a run that had succeeded into an error. Now each
//! book's entry is appended and fsynced **before** its files are touched, and
//! a later line says it completed, so a crash at any point leaves a journal
//! that names every move that may have happened. There is no end-of-run write
//! to fail.
//!
//! # File format
//!
//! `<library>/.lexmechanic/<dir>/<run id>.jsonl`, one JSON object per line:
//!
//! - `{"t":"run","v":{..}}`: the run's header (its struct with no books)
//! - `{"t":"book","v":{..}}`: a book's entry, written before the move. A later
//!   `book` line for the same `book_id` replaces it (retitle writes the
//!   intent first and the finished entry after).
//! - `{"t":"done","book_id":N}`: the book's move completed and was recorded
//! - `{"t":"abandon","book_id":N}`: the move failed and was rolled back
//! - `{"t":"undone","at":T}`: the run was undone
//!
//! A final line cut short by a crash is ignored. [`RunStore::load`] folds the
//! lines into the run's JSON: `books`, `undone_at`, and `pending`, the ids of
//! books that have an entry but neither `done` nor `abandon` (whose move may
//! or may not have happened). Runs written before this existed, as one
//! `<run id>.json`, still load.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};

/// Where one kind of run is kept and how its ids look.
#[derive(Clone, Copy)]
pub(crate) struct RunStore {
    /// Folder under `.lexmechanic`.
    dir_name: &'static str,
    /// Run id prefix.
    prefix: &'static str,
}

pub(crate) const ORGANIZE_RUNS: RunStore = RunStore { dir_name: "organize-runs", prefix: "org" };
pub(crate) const RETITLE_RUNS: RunStore = RunStore { dir_name: "retitle-runs", prefix: "ret" };

/// `<prefix>-<unix seconds>-<4 hex>`.
pub(crate) fn new_run_id(prefix: &str) -> String {
    format!("{prefix}-{}-{}", crate::trash::now_secs(), &uuid::Uuid::new_v4().simple().to_string()[..4])
}

fn valid_run_id(run_id: &str) -> bool {
    !run_id.is_empty() && run_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

impl RunStore {
    pub(crate) fn new_run_id(&self) -> String {
        new_run_id(self.prefix)
    }

    fn dir(&self, library: &Path) -> PathBuf {
        library.join(crate::constants::LIBRARY_HANDLE_DIR_NAME).join(self.dir_name)
    }

    /// Starts a run's journal, header first. Nothing is moved before this has
    /// returned, so an error here is a run that never began.
    pub(crate) fn begin<R: Serialize>(&self, library: &Path, run_id: &str, header: &R) -> Result<RunJournal> {
        if !valid_run_id(run_id) {
            return Err(anyhow!("not a run id"));
        }
        let dir = self.dir(library);
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let path = dir.join(format!("{run_id}.jsonl"));
        let file = OpenOptions::new().create_new(true).append(true).open(&path).with_context(|| format!("creating {}", path.display()))?;
        let mut journal = RunJournal { file, path };
        journal.write_line(&json!({"t": "run", "v": header}), true)?;
        if let Some(parent) = journal.path.parent() {
            sync_dir(parent);
        }
        Ok(journal)
    }

    /// The run as one JSON value, whichever format it was written in.
    fn load_value(&self, library: &Path, run_id: &str) -> Result<Value> {
        if !valid_run_id(run_id) {
            return Err(anyhow!("not a run id"));
        }
        let dir = self.dir(library);
        let journal = dir.join(format!("{run_id}.jsonl"));
        if let Ok(text) = std::fs::read_to_string(&journal) {
            return fold(&text).with_context(|| format!("reading {}", journal.display()));
        }
        let legacy = dir.join(format!("{run_id}.json"));
        let text = std::fs::read_to_string(&legacy).with_context(|| format!("no run {run_id}"))?;
        Ok(serde_json::from_str(&text)?)
    }

    pub(crate) fn load<R: DeserializeOwned>(&self, library: &Path, run_id: &str) -> Result<R> {
        Ok(serde_json::from_value(self.load_value(library, run_id)?)?)
    }

    /// Runs, newest first: `(run_id, created_at, books, undone)`.
    pub(crate) fn list(&self, library: &Path) -> Vec<(String, i64, usize, bool)> {
        let mut out: Vec<(String, i64, usize, bool)> = std::fs::read_dir(self.dir(library))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let path = e.path();
                let ext = path.extension()?.to_str()?;
                if ext != "json" && ext != "jsonl" {
                    return None;
                }
                let run_id = path.file_stem()?.to_str()?.to_string();
                let v = self.load_value(library, &run_id).ok()?;
                Some((
                    v.get("run_id")?.as_str()?.to_string(),
                    v.get("created_at").and_then(Value::as_i64).unwrap_or(0),
                    v.get("books").and_then(Value::as_array).map_or(0, Vec::len),
                    v.get("undone_at").is_some_and(|u| !u.is_null()),
                ))
            })
            .collect();
        out.sort_by_key(|r| std::cmp::Reverse(r.1));
        out
    }

    /// Marks a run undone: a line appended to its journal, or (for a run in
    /// the older whole-file format) that file rewritten.
    pub(crate) fn mark_undone(&self, library: &Path, run_id: &str, at: i64) -> Result<()> {
        if !valid_run_id(run_id) {
            return Err(anyhow!("not a run id"));
        }
        let dir = self.dir(library);
        let journal = dir.join(format!("{run_id}.jsonl"));
        if journal.is_file() {
            let file = OpenOptions::new().append(true).open(&journal)?;
            return RunJournal { file, path: journal }.write_line(&json!({"t": "undone", "at": at}), true);
        }
        let legacy = dir.join(format!("{run_id}.json"));
        let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&legacy)?)?;
        v["undone_at"] = json!(at);
        let temp = legacy.with_extension("json.partial");
        std::fs::write(&temp, serde_json::to_vec_pretty(&v)?)?;
        std::fs::rename(&temp, &legacy)?;
        Ok(())
    }
}

/// An open run journal.
pub(crate) struct RunJournal {
    file: File,
    path: PathBuf,
}

impl RunJournal {
    fn write_line(&mut self, v: &Value, sync: bool) -> Result<()> {
        let mut line = serde_json::to_vec(v)?;
        line.push(b'\n');
        self.file.write_all(&line)?;
        if sync {
            self.file.sync_data()?;
        }
        Ok(())
    }

    /// Writes a book's entry and waits until it is on disk. Call before
    /// touching the book's files.
    pub(crate) fn append_book<B: Serialize>(&mut self, book: &B) -> Result<()> {
        self.write_line(&json!({"t": "book", "v": book}), true)
    }

    /// The book's move completed and is recorded. Not fsynced: if it is lost
    /// the book is merely treated as pending, which undo handles.
    pub(crate) fn mark_done(&mut self, book_id: i32) -> Result<()> {
        self.write_line(&json!({"t": "done", "book_id": book_id}), false)
    }

    /// The book's move failed and was rolled back; it is not part of the run.
    pub(crate) fn abandon(&mut self, book_id: i32) -> Result<()> {
        self.write_line(&json!({"t": "abandon", "book_id": book_id}), false)
    }

    #[allow(dead_code)]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

fn sync_dir(dir: &Path) {
    // Best effort: a directory cannot be opened for sync everywhere.
    if let Ok(f) = File::open(dir) {
        let _ = f.sync_all();
    }
}

/// Folds a journal's lines into the run's JSON.
fn fold(text: &str) -> Result<Value> {
    let mut run: Option<Value> = None;
    let mut books: Vec<Value> = Vec::new();
    let mut pending: Vec<i64> = Vec::new();
    let mut undone_at: Option<i64> = None;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        // A line cut short by a crash is the end of what was written.
        let Ok(v) = serde_json::from_str::<Value>(line) else { break };
        let id_of = |v: &Value| v.get("book_id").and_then(Value::as_i64);
        match v.get("t").and_then(Value::as_str) {
            Some("run") => run = v.get("v").cloned(),
            Some("book") => {
                let Some(book) = v.get("v").cloned() else { continue };
                let id = id_of(&book);
                if let Some(id) = id {
                    if let Some(at) = books.iter().position(|b| id_of(b) == Some(id)) {
                        books.remove(at);
                    }
                    if !pending.contains(&id) {
                        pending.push(id);
                    }
                }
                books.push(book);
            }
            Some("done") => {
                if let Some(id) = id_of(&v) {
                    pending.retain(|p| *p != id);
                }
            }
            Some("abandon") => {
                if let Some(id) = id_of(&v) {
                    pending.retain(|p| *p != id);
                    books.retain(|b| id_of(b) != Some(id));
                }
            }
            Some("undone") => undone_at = v.get("at").and_then(Value::as_i64),
            _ => {}
        }
    }
    let mut run = run.ok_or_else(|| anyhow!("the journal has no header"))?;
    run["books"] = Value::Array(books);
    if let Some(at) = undone_at {
        run["undone_at"] = json!(at);
    }
    if !pending.is_empty() {
        run["pending"] = json!(pending);
    }
    Ok(run)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Default, Serialize, Deserialize)]
    struct Run {
        run_id: String,
        created_at: i64,
        books: Vec<Item>,
        #[serde(default)]
        undone_at: Option<i64>,
        #[serde(default)]
        pending: Vec<i32>,
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Item {
        book_id: i32,
        note: String,
    }

    const STORE: RunStore = RunStore { dir_name: "test-runs", prefix: "tst" };

    fn header(id: &str) -> Run {
        Run { run_id: id.into(), created_at: 7, ..Default::default() }
    }

    #[test]
    fn an_entry_is_on_disk_before_the_move_and_survives_a_crash() {
        let dir = tempfile::tempdir().unwrap();
        let mut j = STORE.begin(dir.path(), "tst-1", &header("tst-1")).unwrap();
        j.append_book(&Item { book_id: 1, note: "a".into() }).unwrap();
        j.mark_done(1).unwrap();
        j.append_book(&Item { book_id: 2, note: "b".into() }).unwrap();
        // The process dies here: no done line for book 2, no end-of-run write.
        drop(j);

        let run: Run = STORE.load(dir.path(), "tst-1").unwrap();
        assert_eq!(run.books.len(), 2, "both moves are in the undo record");
        assert_eq!(run.pending, vec![2], "and the one that may not have finished is flagged");
    }

    #[test]
    fn a_half_written_last_line_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let mut j = STORE.begin(dir.path(), "tst-2", &header("tst-2")).unwrap();
        j.append_book(&Item { book_id: 1, note: "a".into() }).unwrap();
        j.mark_done(1).unwrap();
        let path = j.path().to_path_buf();
        drop(j);
        let mut f = OpenOptions::new().append(true).open(path).unwrap();
        f.write_all(b"{\"t\":\"book\",\"v\":{\"book_id\":2,\"no").unwrap();

        let run: Run = STORE.load(dir.path(), "tst-2").unwrap();
        assert_eq!(run.books.len(), 1);
        assert!(run.pending.is_empty());
    }

    #[test]
    fn a_later_entry_replaces_an_earlier_one_and_an_abandoned_one_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let mut j = STORE.begin(dir.path(), "tst-3", &header("tst-3")).unwrap();
        j.append_book(&Item { book_id: 1, note: "intent".into() }).unwrap();
        j.append_book(&Item { book_id: 1, note: "final".into() }).unwrap();
        j.mark_done(1).unwrap();
        j.append_book(&Item { book_id: 2, note: "never moved".into() }).unwrap();
        j.abandon(2).unwrap();
        drop(j);
        let run: Run = STORE.load(dir.path(), "tst-3").unwrap();
        assert_eq!(run.books, vec![Item { book_id: 1, note: "final".into() }]);
        assert!(run.pending.is_empty());
    }

    #[test]
    fn marking_undone_appends_and_lists_find_every_run() {
        let dir = tempfile::tempdir().unwrap();
        let mut j = STORE.begin(dir.path(), "tst-4", &header("tst-4")).unwrap();
        j.append_book(&Item { book_id: 1, note: "a".into() }).unwrap();
        j.mark_done(1).unwrap();
        STORE.mark_undone(dir.path(), "tst-4", 99).unwrap();
        let run: Run = STORE.load(dir.path(), "tst-4").unwrap();
        assert_eq!(run.undone_at, Some(99));
        assert_eq!(STORE.list(dir.path()), vec![("tst-4".to_string(), 7, 1, true)]);
    }

    #[test]
    fn a_run_in_the_older_whole_file_format_still_loads_and_can_be_marked_undone() {
        let dir = tempfile::tempdir().unwrap();
        let runs = dir.path().join(crate::constants::LIBRARY_HANDLE_DIR_NAME).join("test-runs");
        std::fs::create_dir_all(&runs).unwrap();
        std::fs::write(runs.join("tst-old.json"), r#"{"run_id":"tst-old","created_at":3,"books":[{"book_id":5,"note":"x"}]}"#).unwrap();
        let run: Run = STORE.load(dir.path(), "tst-old").unwrap();
        assert_eq!(run.books.len(), 1);
        STORE.mark_undone(dir.path(), "tst-old", 50).unwrap();
        assert_eq!(STORE.load::<Run>(dir.path(), "tst-old").unwrap().undone_at, Some(50));
    }

    #[test]
    fn a_run_id_cannot_name_another_path() {
        let dir = tempfile::tempdir().unwrap();
        assert!(STORE.load::<Run>(dir.path(), "../x").is_err());
        assert!(STORE.begin(dir.path(), "a/b", &header("a")).is_err());
    }
}
