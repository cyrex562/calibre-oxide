//! Making the change log whole again after it fell behind `metadata.db`.
//!
//! # When this is needed
//!
//! A write reaches `metadata.db` first and the log second. When the second
//! step fails (a full disk, an unwritable `changes/` folder, a share that went
//! away) the write is kept and a marker is left behind ([`Cache::change_log_gap`]).
//! From then on the log is missing changes that only the database has, so a
//! rebuild from it would lose them, and [`Cache::rebuild_from_change_log`]
//! refuses.
//!
//! Deleting the marker would silence that warning without fixing anything. The
//! repair here fixes it: it replays the log into memory, compares that
//! state with what the database holds *now*, and appends one batch of changes
//! covering exactly the difference. Once those are durable, a rebuild from the
//! log produces the database as it is today, and the marker can go.
//!
//! # Why a difference and not a fresh snapshot of everything
//!
//! A snapshot of a 100,000-book library is every field of every book, tens of
//! megabytes written to a share that has just been failing. The difference is
//! usually a handful of books, and it is the same kind of entry any write
//! appends, so replay, compaction and verification need no new code path.
//! A log that is already in step costs one replay and writes nothing.
//!
//! # What is compared
//!
//! Books (add and remove), the standard fields, formats (name and size),
//! the has-cover flag, custom column definitions and values, and
//! preferences: everything a rebuild restores.
//!
//! # Sort keys
//!
//! The database derives `books.sort` from the title and `authors.sort` from the
//! author's name, and a replay re-derives them. So a sort key is compared with
//! what a replay *would* give it (the logged value, or the derived one), not
//! with whatever the log happens to hold: a key set straight in the database,
//! which never reached the log, shows up as a difference and is appended
//! explicitly. A library whose keys are all derived appends nothing.

use std::collections::{HashMap, HashSet};

use anyhow::Result;

use crate::cache::Cache;
use crate::change_log::ChangeOp;

/// The standard fields a book's log state is compared on, as `set_field` names
/// them. `authors` comes before `author_sort`: setting the authors re-derives the sort, so a
/// hand-set sort has to be replayed after it (the order `book_added_ops` uses too). `uuid` is the book's identity and `last_modified` is stamped by the
/// database itself, so neither is replayed.
const FIELDS: &[&str] = &[
    "title", "sort", "authors", "author_sort", "path", "series", "series_index", "publisher", "rating", "tags", "languages", "comments", "isbn", "identifiers", "timestamp", "pubdate",
];

/// How many ops go into one logged batch entry.
const BATCH: usize = 500;

/// What a repair appended.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RepairReport {
    /// Books the database has that the log did not know.
    pub books_added: usize,
    /// Books the log still had that the database no longer does.
    pub books_removed: usize,
    /// Everything else that differed: fields, formats, covers, columns, preferences.
    pub other_changes: usize,
    /// Whether the "behind" marker was cleared. `false` when a write failed to be
    /// logged *during* the repair, which leaves the marker for the next one.
    pub marker_cleared: bool,
}

/// What the log says one book looks like.
#[derive(Default)]
struct LoggedBook {
    fields: HashMap<String, Option<String>>,
    /// Format (upper case) to `(name, size)`.
    formats: HashMap<String, (String, i64)>,
    cover: bool,
    /// The title a replay would have set by now (`None` is the `Unknown` a
    /// fresh row starts with). The database re-derives `sort` whenever the
    /// title *changes*, so a title set after a `sort` discards that `sort`.
    title: Option<String>,
}

/// What replaying the log would produce, as far as the repair compares it.
#[derive(Default)]
struct LoggedState {
    books: HashMap<String, LoggedBook>,
    columns: HashSet<String>,
    prefs: HashMap<String, Option<String>>,
    /// Author name to the sort key logged for it.
    author_sorts: HashMap<String, String>,
}

impl LoggedState {
    fn absorb(&mut self, op: &ChangeOp) {
        match op {
            ChangeOp::Batch { ops } => ops.iter().for_each(|o| self.absorb(o)),
            ChangeOp::BookAdded { book } => {
                self.books.entry(book.clone()).or_default();
            }
            ChangeOp::BookRemoved { book } => {
                self.books.remove(book);
            }
            ChangeOp::FieldSet { book, field, value } => {
                if let Some(b) = self.books.get_mut(book) {
                    // An empty title or sort is a no-op on replay.
                    if matches!(field.as_str(), "title" | "sort") && value.as_deref().unwrap_or("").is_empty() {
                        return;
                    }
                    if field == "title" {
                        let new = value.as_deref().unwrap_or("");
                        if new != b.title.as_deref().unwrap_or("Unknown") {
                            b.fields.remove("sort");
                        }
                        b.title = Some(new.to_string());
                    }
                    if field == "has_cover" || field == "cover" {
                        b.cover = matches!(value.as_deref(), Some("1" | "true" | "True"));
                    } else {
                        b.fields.insert(field.strip_prefix('#').unwrap_or(field).to_string(), value.clone());
                    }
                }
            }
            ChangeOp::FormatSet { book, format, name, size, .. } => {
                if let Some(b) = self.books.get_mut(book) {
                    b.formats.insert(format.to_uppercase(), (name.clone(), *size));
                }
            }
            ChangeOp::FormatRemoved { book, format } => {
                if let Some(b) = self.books.get_mut(book) {
                    b.formats.remove(&format.to_uppercase());
                }
            }
            ChangeOp::CoverSet { book, blob } => {
                if let Some(b) = self.books.get_mut(book) {
                    b.cover = blob.is_some();
                }
            }
            ChangeOp::PrefSet { key, value } => {
                self.prefs.insert(key.clone(), value.clone());
            }
            ChangeOp::CustomColumnAdded { label, .. } => {
                self.columns.insert(label.clone());
            }
            ChangeOp::CustomColumnRemoved { label } => {
                self.columns.remove(label);
            }
            // A rename composes with the field values already logged; the
            // comparison below simply re-states any value it changed.
            ChangeOp::ItemRenamed { kind, from, to } => {
                // The database re-derives an author's sort key from the new name.
                if kind == "authors" {
                    self.author_sorts.remove(from);
                    self.author_sorts.remove(to);
                }
            }
            ChangeOp::AuthorSortSet { name, sort } => {
                self.author_sorts.insert(name.clone(), sort.clone());
            }
        }
    }
}

/// Whether two logged values mean the same field value. Numbers compare as numbers
/// (`1` and `1.0` are the same series index).
fn same_value(field: &str, logged: &str, current: &str) -> bool {
    if logged == current {
        return true;
    }
    if matches!(field, "series_index" | "rating") {
        if let (Ok(a), Ok(b)) = (logged.parse::<f64>(), current.parse::<f64>()) {
            return a == b;
        }
    }
    false
}

impl Cache {
    /// Appends to the change log whatever `metadata.db` has that the log lacks
    /// (see the module docs), then clears the "behind" marker.
    ///
    /// Safe to run at any time and idempotent: with the log already in step it
    /// appends nothing. The marker is cleared only after every change is durable
    /// and only if no further append failed in the meantime; on any error it stays.
    pub fn repair_change_log(&self) -> Result<RepairReport> {
        let marker_before = self.change_log_gap();
        let log = self.backend.change_log()?;
        let mut logged = LoggedState::default();
        for change in log.replay()?.changes() {
            logged.absorb(&change.op);
        }

        let mut report = RepairReport::default();
        let mut ops: Vec<ChangeOp> = Vec::new();

        // Custom columns first: a value can only be replayed into a column that exists.
        let columns = self.custom_column_label_map()?;
        let mut labels: Vec<&String> = columns.keys().collect();
        labels.sort();
        for label in &labels {
            if !logged.columns.contains(*label) {
                let c = &columns[*label];
                let display = c["display"].as_object().filter(|d| !d.is_empty()).map(|_| c["display"].to_string());
                ops.push(ChangeOp::CustomColumnAdded {
                    label: (*label).clone(),
                    name: c["name"].as_str().unwrap_or(label).to_string(),
                    datatype: c["datatype"].as_str().unwrap_or("text").to_string(),
                    is_multiple: c["is_multiple"].as_bool().unwrap_or(false),
                    display,
                });
                report.other_changes += 1;
            }
        }
        let mut gone: Vec<&String> = logged.columns.iter().filter(|l| !columns.contains_key(*l)).collect();
        gone.sort();
        for label in gone {
            ops.push(ChangeOp::CustomColumnRemoved { label: label.clone() });
            report.other_changes += 1;
        }

        // Books the database has: every field, format and cover that differs.
        let hashes = self.checksums().all_format_records().unwrap_or_default();
        let formats = self.all_format_rows()?;
        let mut present: HashSet<String> = HashSet::new();
        for id in self.all_book_ids()? {
            let Some(uuid) = self.book_uuid(id)? else { continue };
            present.insert(uuid.clone());
            let empty = LoggedBook::default();
            let known = logged.books.get(&uuid);
            if known.is_none() {
                ops.push(ChangeOp::BookAdded { book: uuid.clone() });
                report.books_added += 1;
            }
            let was = known.unwrap_or(&empty);

            let mut authors_restated = false;
            for field in FIELDS {
                let current = self.field_for(id, field)?.unwrap_or_default();
                if *field == "sort" {
                    // What a replay leaves in `sort`: the logged key if nothing since
                    // changed the title, otherwise the key derived from the title.
                    if current.is_empty() {
                        continue;
                    }
                    let title = self.field_for(id, "title")?.unwrap_or_default();
                    let title_changes = !title.is_empty() && title != was.title.as_deref().unwrap_or("Unknown");
                    let expected = match was.fields.get("sort").cloned().flatten() {
                        Some(logged) if !title_changes => logged,
                        _ => self.derived_title_sort(&title)?,
                    };
                    if current != expected {
                        ops.push(ChangeOp::FieldSet { book: uuid.clone(), field: "sort".to_string(), value: Some(current) });
                        if known.is_some() {
                            report.other_changes += 1;
                        }
                    }
                    continue;
                }
                let before = was.fields.get(*field).cloned().flatten().unwrap_or_default();
                // An empty title is a no-op on replay, so an empty value there is not a difference.
                // Replaying new authors re-derives the sort, which then no longer matches the
                // database's even when the logged sort equals it: state it again after the authors.
                let forced = *field == "author_sort" && authors_restated && !current.is_empty();
                if !forced && (same_value(field, &before, &current) || (current.is_empty() && *field == "title")) {
                    continue;
                }
                if *field == "authors" {
                    authors_restated = true;
                }
                ops.push(ChangeOp::FieldSet { book: uuid.clone(), field: (*field).to_string(), value: Some(current) });
                if known.is_some() {
                    report.other_changes += 1;
                }
            }

            let rows = formats.get(&id).cloned().unwrap_or_default();
            for (format, name, size) in &rows {
                if was.formats.get(format) != Some(&(name.clone(), *size)) {
                    let hash = hashes.get(&(id, format.clone())).map(|(h, _, _)| h.clone());
                    ops.push(ChangeOp::FormatSet { book: uuid.clone(), format: format.clone(), name: name.clone(), size: *size, hash });
                    if known.is_some() {
                        report.other_changes += 1;
                    }
                }
            }
            for format in was.formats.keys() {
                if !rows.iter().any(|(f, _, _)| f == format) {
                    ops.push(ChangeOp::FormatRemoved { book: uuid.clone(), format: format.clone() });
                    report.other_changes += 1;
                }
            }

            let has_cover = self.has_cover_flag(id)?;
            if has_cover != was.cover {
                ops.push(ChangeOp::CoverSet { book: uuid.clone(), blob: has_cover.then(|| uuid.clone()) });
                if known.is_some() {
                    report.other_changes += 1;
                }
            }

            for label in &labels {
                let mut keys = vec![(*label).clone()];
                if columns[*label]["datatype"] == "series" {
                    keys.push(format!("{label}_index"));
                }
                for key in keys {
                    let current = self.get_custom_column_value(id, &key)?.unwrap_or_default();
                    let before = was.fields.get(&key).cloned().flatten().unwrap_or_default();
                    if !same_value("", &before, &current) {
                        ops.push(ChangeOp::FieldSet { book: uuid.clone(), field: format!("#{key}"), value: Some(current) });
                        if known.is_some() {
                            report.other_changes += 1;
                        }
                    }
                }
            }
        }

        // Author sort keys, after every book op so the authors exist when they replay.
        for (name, sort) in self.all_author_sorts()? {
            let expected = match logged.author_sorts.get(&name) {
                Some(logged) => logged.clone(),
                None => self.derived_author_sort(&name)?,
            };
            if sort != expected {
                ops.push(ChangeOp::AuthorSortSet { name, sort });
                report.other_changes += 1;
            }
        }

        // Books the log still has but the database does not.
        let mut removed: Vec<&String> = logged.books.keys().filter(|u| !present.contains(*u)).collect();
        removed.sort();
        for uuid in removed {
            ops.push(ChangeOp::BookRemoved { book: uuid.clone() });
            report.books_removed += 1;
        }

        // Preferences.
        let current_prefs = self.all_preferences()?;
        let mut keys: Vec<&String> = current_prefs.keys().collect();
        keys.sort();
        for key in keys {
            if logged.prefs.get(key).cloned().flatten().as_deref() != Some(current_prefs[key].as_str()) {
                ops.push(ChangeOp::PrefSet { key: key.clone(), value: Some(current_prefs[key].clone()) });
                report.other_changes += 1;
            }
        }
        let mut stale: Vec<&String> = logged.prefs.iter().filter(|(k, v)| v.is_some() && !current_prefs.contains_key(*k)).map(|(k, _)| k).collect();
        stale.sort();
        for key in stale {
            ops.push(ChangeOp::PrefSet { key: key.clone(), value: None });
            report.other_changes += 1;
        }

        // Straight to the log, not through `record`: a failure here must come
        // back as an error and leave the marker alone.
        for chunk in ops.chunks(BATCH) {
            log.append(ChangeOp::Batch { ops: chunk.to_vec() })?;
        }

        // A write that failed to log while this ran added to the marker; its
        // change may be newer than what was compared, so the marker stays.
        let marker_now = self.change_log_gap();
        if marker_now == marker_before {
            self.clear_change_log_gap()?;
            report.marker_cleared = true;
        }
        Ok(report)
    }

    /// Every `data` row as `book -> [(FORMAT, name, size)]`, in one query.
    fn all_format_rows(&self) -> Result<HashMap<i32, Vec<(String, String, i64)>>> {
        let conn = self.backend.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT book, format, name, uncompressed_size FROM data")?;
        let mut out: HashMap<i32, Vec<(String, String, i64)>> = HashMap::new();
        for row in stmt.query_map([], |r| Ok((r.get::<_, i32>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?)))? {
            let (book, format, name, size) = row?;
            out.entry(book).or_default().push((format.to_uppercase(), name, size));
        }
        Ok(out)
    }

    /// `title_sort(title)`, the key the database's triggers derive.
    fn derived_title_sort(&self, title: &str) -> Result<String> {
        let conn = self.backend.conn.lock().unwrap();
        Ok(conn.query_row("SELECT title_sort(?1)", [title], |r| r.get(0))?)
    }

    /// `author_to_author_sort(name)`, the key the database's triggers derive.
    fn derived_author_sort(&self, name: &str) -> Result<String> {
        let conn = self.backend.conn.lock().unwrap();
        Ok(conn.query_row("SELECT author_to_author_sort(?1)", [name], |r| r.get(0))?)
    }

    /// Every author as `(name, stored sort key)`, sorted by name.
    fn all_author_sorts(&self) -> Result<Vec<(String, String)>> {
        let conn = self.backend.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT name, sort FROM authors ORDER BY name")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default())))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    fn has_cover_flag(&self, book_id: i32) -> Result<bool> {
        let conn = self.backend.conn.lock().unwrap();
        Ok(conn.query_row("SELECT has_cover FROM books WHERE id = ?1", [book_id], |r| r.get::<_, i64>(0)).unwrap_or(0) != 0)
    }

    fn all_preferences(&self) -> Result<HashMap<String, String>> {
        let conn = self.backend.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT key, val FROM preferences")?;
        let mut out = HashMap::new();
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))? {
            let (k, v) = row?;
            if let Some(v) = v {
                out.insert(k, v);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache() -> (tempfile::TempDir, Cache) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        (dir, cache)
    }

    fn add(dir: &std::path::Path, cache: &Cache, title: &str) -> i32 {
        let source = dir.join(format!("{title}.pdf"));
        std::fs::write(&source, title.as_bytes()).unwrap();
        let mut meta = lexmechanic_ebooks::metadata::MetaInformation::default();
        meta.title = title.to_string();
        meta.authors = vec!["An Author".to_string()];
        cache.add_book(&source, &meta).unwrap()
    }

    /// Titles, authors, tags and formats of every book, sorted: what a rebuild must reproduce.
    fn view(cache: &Cache) -> Vec<(String, String, String, String)> {
        let mut out: Vec<_> = cache
            .all_book_ids()
            .unwrap()
            .into_iter()
            .map(|id| {
                (
                    cache.field_for(id, "title").unwrap().unwrap_or_default(),
                    cache.field_for(id, "authors").unwrap().unwrap_or_default(),
                    cache.field_for(id, "tags").unwrap().unwrap_or_default(),
                    cache.field_for(id, "formats").unwrap().unwrap_or_default(),
                )
            })
            .collect();
        out.sort();
        out
    }

    /// Copies only the change log into a new library and rebuilds from it.
    fn rebuilt_from_log_only(from: &std::path::Path) -> (tempfile::TempDir, Cache) {
        let to = tempfile::tempdir().unwrap();
        let handle = crate::constants::LIBRARY_HANDLE_DIR_NAME;
        for sub in ["changes", "snapshots"] {
            let src = from.join(handle).join(sub);
            let dst = to.path().join(handle).join(sub);
            std::fs::create_dir_all(&dst).unwrap();
            for entry in std::fs::read_dir(&src).into_iter().flatten().flatten() {
                std::fs::copy(entry.path(), dst.join(entry.file_name())).unwrap();
            }
        }
        let cache = Cache::new(to.path()).unwrap();
        cache.rebuild_from_change_log().unwrap();
        (to, cache)
    }

    #[test]
    fn a_log_that_is_already_in_step_gets_nothing_appended() {
        let (dir, cache) = cache();
        add(dir.path(), &cache, "One");
        let log = cache.backend.change_log().unwrap();
        let before = log.entry_count();
        let report = cache.repair_change_log().unwrap();
        assert_eq!((report.books_added, report.books_removed, report.other_changes), (0, 0, 0), "{report:?}");
        assert_eq!(log.entry_count(), before, "nothing to append");
    }

    #[test]
    fn repair_covers_changes_the_log_never_received_and_clears_the_marker() {
        let (dir, cache) = cache();
        let kept = add(dir.path(), &cache, "Kept");
        let doomed = add(dir.path(), &cache, "Doomed");

        // Writes whose log append fails: made straight in the database, as a failed append leaves them,
        // then the marker a failed append writes.
        {
            let conn = cache.backend.conn.lock().unwrap();
            conn.execute("UPDATE books SET title = 'Kept, retitled' WHERE id = ?1", [kept]).unwrap();
            conn.execute("DELETE FROM books WHERE id = ?1", [doomed]).unwrap();
        }
        cache.note_change_log_gap("disk full");
        assert!(cache.change_log_gap().is_some());
        assert!(cache.rebuild_from_change_log().is_err(), "a rebuild is refused while the log is behind");

        let report = cache.repair_change_log().unwrap();
        assert!(report.marker_cleared, "{report:?}");
        assert!(cache.change_log_gap().is_none());
        assert!(report.books_removed >= 1, "{report:?}");

        // The log alone now rebuilds the library as it is today.
        let (_keep, rebuilt) = rebuilt_from_log_only(dir.path());
        assert_eq!(view(&rebuilt), view(&cache));
        assert!(view(&rebuilt).iter().any(|(t, ..)| t == "Kept, retitled"));
        assert!(!view(&rebuilt).iter().any(|(t, ..)| t == "Doomed"));
    }

    #[test]
    fn a_book_missing_from_the_log_is_added_with_its_format() {
        let (dir, cache) = cache();
        add(dir.path(), &cache, "Logged");
        // A book inserted without a log entry (a failed append leaves exactly this).
        let id = {
            let conn = cache.backend.conn.lock().unwrap();
            conn.execute("INSERT INTO books (title, author_sort, path, has_cover, series_index, uuid) VALUES ('Silent', 'S', 'silent', 0, 1.0, 'uuid-silent')", ()).unwrap();
            let id = conn.last_insert_rowid() as i32;
            conn.execute("INSERT INTO data (book, format, uncompressed_size, name) VALUES (?1, 'PDF', 42, 'Silent')", [id]).unwrap();
            id
        };
        let _ = id;
        cache.note_change_log_gap("io error");
        let report = cache.repair_change_log().unwrap();
        assert_eq!(report.books_added, 1, "{report:?}");
        let (_keep, rebuilt) = rebuilt_from_log_only(dir.path());
        let silent = rebuilt.all_book_ids().unwrap().into_iter().find(|b| rebuilt.field_for(*b, "title").unwrap().as_deref() == Some("Silent")).expect("the missed book is in the rebuilt library");
        assert_eq!(rebuilt.field_for(silent, "path").unwrap().as_deref(), Some("silent"));
        assert_eq!(rebuilt.field_for(silent, "size").unwrap().as_deref(), Some("42"));
    }

    fn sorts(cache: &Cache) -> Vec<(String, String, String)> {
        let mut out: Vec<_> = cache
            .all_book_ids()
            .unwrap()
            .into_iter()
            .map(|id| (cache.field_for(id, "title").unwrap().unwrap_or_default(), cache.field_for(id, "sort").unwrap().unwrap_or_default(), cache.field_for(id, "author_sort").unwrap().unwrap_or_default()))
            .collect();
        out.sort();
        out
    }

    #[test]
    fn a_hand_set_sort_is_found_and_a_rebuild_reproduces_it() {
        let (dir, cache) = cache();
        let plain = add(dir.path(), &cache, "The Plain One");
        let edited = add(dir.path(), &cache, "Edited");
        // Set straight in the database: the write never reached the log.
        {
            let conn = cache.backend.conn.lock().unwrap();
            conn.execute("UPDATE books SET sort = 'Zzz Hand Set' WHERE id = ?1", [edited]).unwrap();
            conn.execute("UPDATE books SET author_sort = 'Hand, Author' WHERE id = ?1", [edited]).unwrap();
        }
        let _ = plain;
        let report = cache.repair_change_log().unwrap();
        assert!(report.other_changes >= 2, "{report:?}");
        let (_keep, rebuilt) = rebuilt_from_log_only(dir.path());
        assert_eq!(sorts(&rebuilt), sorts(&cache));
        assert!(sorts(&rebuilt).iter().any(|(t, s, a)| t == "Edited" && s == "Zzz Hand Set" && a == "Hand, Author"));
        assert!(sorts(&rebuilt).iter().any(|(t, s, _)| t == "The Plain One" && s == "Plain One, The"), "derived keys stay derived");
        // Idempotent: the log is now in step.
        let log = cache.backend.change_log().unwrap();
        let before = log.entry_count();
        cache.repair_change_log().unwrap();
        assert_eq!(log.entry_count(), before);
    }

    #[test]
    fn a_hand_set_sort_next_to_a_logged_one_is_found_too() {
        let (dir, cache) = cache();
        let id = add(dir.path(), &cache, "Logged Sort");
        cache.set_field(id, "sort", "Logged Key").unwrap();
        {
            let conn = cache.backend.conn.lock().unwrap();
            conn.execute("UPDATE books SET sort = 'Raw Key' WHERE id = ?1", [id]).unwrap();
        }
        cache.repair_change_log().unwrap();
        let (_keep, rebuilt) = rebuilt_from_log_only(dir.path());
        assert_eq!(sorts(&rebuilt), sorts(&cache));
        assert!(sorts(&rebuilt).iter().any(|(_, s, _)| s == "Raw Key"));
    }

    #[test]
    fn a_hand_set_author_sort_key_is_found_and_a_rebuild_reproduces_it() {
        let (dir, cache) = cache();
        add(dir.path(), &cache, "By Someone");
        {
            let conn = cache.backend.conn.lock().unwrap();
            conn.execute("UPDATE authors SET sort = 'Author, Hand Set' WHERE name = 'An Author'", []).unwrap();
        }
        let report = cache.repair_change_log().unwrap();
        assert!(report.other_changes >= 1, "{report:?}");
        let (_keep, rebuilt) = rebuilt_from_log_only(dir.path());
        assert_eq!(rebuilt.author_sort_for_name("An Author").unwrap().as_deref(), Some("Author, Hand Set"));
    }

    /// A foreign writer changed a book's authors and hand-set its `author_sort`: replaying
    /// `authors` re-derives the sort, so the hand-set one has to come after it.
    #[test]
    fn a_hand_set_author_sort_survives_a_replay_of_changed_authors() {
        let (dir, cache) = cache();
        let id = add(dir.path(), &cache, "Re-authored");
        {
            let conn = cache.backend.conn.lock().unwrap();
            conn.execute("INSERT INTO authors (name, sort) VALUES ('New Writer', 'Writer, New')", []).unwrap();
            conn.execute("UPDATE books_authors_link SET author = (SELECT id FROM authors WHERE name = 'New Writer') WHERE book = ?1", [id]).unwrap();
            conn.execute("UPDATE books SET author_sort = 'Hand, Set' WHERE id = ?1", [id]).unwrap();
        }
        cache.repair_change_log().unwrap();
        let (_keep, rebuilt) = rebuilt_from_log_only(dir.path());
        assert_eq!(view(&rebuilt), view(&cache));
        assert_eq!(sorts(&rebuilt), sorts(&cache));
        assert!(sorts(&rebuilt).iter().any(|(t, _, a)| t == "Re-authored" && a == "Hand, Set"), "{:?}", sorts(&rebuilt));
        let log = cache.backend.change_log().unwrap();
        let before = log.entry_count();
        cache.repair_change_log().unwrap();
        assert_eq!(log.entry_count(), before, "idempotent");
    }

    #[test]
    fn a_library_with_only_derived_and_logged_sort_keys_gets_nothing_appended() {
        let (dir, cache) = cache();
        let a = add(dir.path(), &cache, "The Hobbit");
        let b = add(dir.path(), &cache, "A Tale");
        cache.set_field(a, "sort", "Hobbit Special").unwrap();
        // A title change after a hand-set sort re-derives it, in the database and in a replay.
        cache.set_field(b, "sort", "Tale Special").unwrap();
        cache.set_field(b, "title", "An Unusual Tale").unwrap();
        cache.set_field(a, "author_sort", "Author, An").unwrap();
        let log = cache.backend.change_log().unwrap();
        let before = log.entry_count();
        let report = cache.repair_change_log().unwrap();
        assert_eq!((report.books_added, report.books_removed, report.other_changes), (0, 0, 0), "{report:?}");
        assert_eq!(log.entry_count(), before, "nothing to append");
    }
}
