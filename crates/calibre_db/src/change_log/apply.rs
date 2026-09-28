//! Applying logged changes back to `metadata.db` (issue #901, part of
//! #899).
//!
//! This is the half that makes the log authoritative rather than merely
//! recorded: if every mutation is in the log, and the log can be
//! applied to an empty database, then `metadata.db` is a cache and
//! deleting it is a recovery step.
//!
//! # uuid to id
//!
//! Changes name books by uuid; the database keys them by a local
//! autoincrement `id`. Every apply therefore starts by resolving one to
//! the other, and [`ChangeOp::BookAdded`] is what creates the mapping.
//!
//! A change for an unknown uuid is **skipped, not invented**. It means
//! the `BookAdded` that should have preceded it has not arrived — which
//! is possible while a peer's changes are still syncing in — and
//! guessing would attach somebody else's metadata to whichever book
//! happened to be first.
//!
//! # Appends are suppressed during a replay
//!
//! The write paths this calls append to the log. Left alone they would
//! append the very entries they are being fed, doubling the log on
//! every rebuild. [`Backend::begin_replay`] holds that off, via a guard
//! so it cannot be left switched on.

use anyhow::Result;

use crate::cache::Cache;

use super::change::{Change, ChangeOp};
use super::store::ChangeLog;

/// What a replay did, and what it could not do.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReplayReport {
    pub applied: usize,
    /// Changes for a book whose `BookAdded` has not arrived yet.
    pub skipped_unknown_book: usize,
}

/// Rebuilds `cache`'s tables from `log`.
///
/// Intended for a database that is empty or being discarded: applying
/// over existing rows is safe (every op is idempotent by construction —
/// each one sets a value rather than adjusting it) but pointless.
pub fn replay_into(cache: &Cache, log: &ChangeLog) -> Result<ReplayReport> {
    let replay = log.replay()?;
    let _guard = cache.backend.begin_replay();

    let mut report = ReplayReport::default();
    for change in replay.changes() {
        if apply(cache, change)? {
            report.applied += 1;
        } else {
            report.skipped_unknown_book += 1;
        }
    }
    Ok(report)
}

/// Applies one change. Returns whether it could be applied at all.
pub fn apply(cache: &Cache, change: &Change) -> Result<bool> {
    match &change.op {
        ChangeOp::BookAdded { book } => {
            // Already there: replaying a log twice, or a peer's add for
            // a book this machine also knows.
            if cache.book_id_for_uuid(book)?.is_some() {
                return Ok(true);
            }
            cache.insert_book_with_uuid(book)?;
            Ok(true)
        }
        ChangeOp::BookRemoved { book } => match cache.book_id_for_uuid(book)? {
            Some(id) => {
                cache.delete_book(id)?;
                Ok(true)
            }
            // Removing a book that is not here is the desired end state
            // already, so this counts as applied rather than skipped.
            None => Ok(true),
        },
        ChangeOp::FieldSet { book, field, value } => match cache.book_id_for_uuid(book)? {
            Some(id) => {
                let value = value.as_deref().unwrap_or("");
                // `path` is structural -- where the book's files live --
                // and `set_field` refuses it on purpose, so that a bulk
                // metadata edit cannot move books around. Replay still
                // has to restore it, through its own named method.
                if field == "path" {
                    cache.set_book_path(id, value)?;
                } else {
                    cache.set_field(id, field, value)?;
                }
                Ok(true)
            }
            None => Ok(false),
        },
        ChangeOp::FormatSet { book, format, name, size, hash } => match cache.book_id_for_uuid(book)? {
            Some(id) => {
                cache.record_format_row(id, format, name, *size, hash.as_deref())?;
                Ok(true)
            }
            None => Ok(false),
        },
        ChangeOp::FormatRemoved { book, format } => match cache.book_id_for_uuid(book)? {
            Some(id) => {
                cache.forget_format_row(id, format)?;
                Ok(true)
            }
            None => Ok(true),
        },
        ChangeOp::CoverSet { book, blob } => match cache.book_id_for_uuid(book)? {
            Some(id) => {
                cache.set_field(id, "has_cover", if blob.is_some() { "1" } else { "0" })?;
                Ok(true)
            }
            None => Ok(false),
        },

        // Library-scoped from here down: nothing to resolve a uuid
        // against, so these always apply.
        ChangeOp::PrefSet { key, value } => {
            match value {
                Some(value) => cache.set_preference(key, value)?,
                None => cache.clear_preference(key)?,
            }
            Ok(true)
        }
        ChangeOp::CustomColumnAdded { label, name, datatype, is_multiple } => {
            // Replaying a log twice, or a peer's add for a column this
            // machine already has -- the same already-there case
            // `BookAdded` handles, and `add_custom_column` errors on a
            // duplicate label rather than ignoring it.
            if cache.custom_column_id(label)?.is_some() {
                return Ok(true);
            }
            cache.add_custom_column(label, name, datatype, *is_multiple)?;
            Ok(true)
        }
        ChangeOp::CustomColumnRemoved { label } => {
            if cache.custom_column_id(label)?.is_none() {
                // Already the desired end state.
                return Ok(true);
            }
            cache.remove_custom_column(label)?;
            Ok(true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change_log::ChangeOp;

    fn cache() -> (tempfile::TempDir, Cache) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        (dir, cache)
    }

    #[test]
    fn book_added_creates_a_row_with_that_uuid() {
        let (_dir, cache) = cache();
        let log = cache.backend.change_log().unwrap();
        let change = log.append(ChangeOp::BookAdded { book: "uuid-1".into() }).unwrap();

        assert!(apply(&cache, &change).unwrap());
        let id = cache.book_id_for_uuid("uuid-1").unwrap().expect("the book should exist");
        assert_eq!(cache.field_for(id, "uuid").unwrap().as_deref(), Some("uuid-1"));
    }

    #[test]
    fn applying_the_same_add_twice_does_not_duplicate_the_book() {
        let (_dir, cache) = cache();
        let log = cache.backend.change_log().unwrap();
        let change = log.append(ChangeOp::BookAdded { book: "uuid-1".into() }).unwrap();

        apply(&cache, &change).unwrap();
        apply(&cache, &change).unwrap();

        let count: i64 = cache.backend.conn.lock().unwrap().query_row("SELECT COUNT(*) FROM books", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 1, "replaying a log twice must not double the library");
    }

    /// The case that makes uuid the right key: a change for a book this
    /// machine has never heard of must be left alone, not applied to
    /// whichever book happens to be there.
    #[test]
    fn a_change_for_an_unknown_book_is_skipped_not_misapplied() {
        let (_dir, cache) = cache();
        let log = cache.backend.change_log().unwrap();
        let added = log.append(ChangeOp::BookAdded { book: "known".into() }).unwrap();
        apply(&cache, &added).unwrap();
        let known = cache.book_id_for_uuid("known").unwrap().unwrap();

        let stray = log.append(ChangeOp::FieldSet { book: "never-seen".into(), field: "title".into(), value: Some("Not Mine".into()) }).unwrap();
        assert!(!apply(&cache, &stray).unwrap(), "should report that it could not apply");
        assert_ne!(cache.field_for(known, "title").unwrap().as_deref(), Some("Not Mine"));
    }

    #[test]
    fn removing_a_book_that_is_not_here_is_already_the_desired_state() {
        let (_dir, cache) = cache();
        let log = cache.backend.change_log().unwrap();
        let change = log.append(ChangeOp::BookRemoved { book: "never-existed".into() }).unwrap();
        assert!(apply(&cache, &change).unwrap());
    }

    #[test]
    fn a_replay_reports_what_it_could_not_apply() {
        let (_dir, cache) = cache();
        let log = cache.backend.change_log().unwrap();
        log.append(ChangeOp::BookAdded { book: "u1".into() }).unwrap();
        log.append(ChangeOp::FieldSet { book: "u1".into(), field: "title".into(), value: Some("Dune".into()) }).unwrap();
        log.append(ChangeOp::FieldSet { book: "orphan".into(), field: "title".into(), value: Some("?".into()) }).unwrap();

        let report = replay_into(&cache, &log).unwrap();
        assert_eq!(report.applied, 2);
        assert_eq!(report.skipped_unknown_book, 1);
    }

    /// Everything about a book that a rebuild has to reproduce.
    fn snapshot(cache: &Cache) -> Vec<(String, String, String, String, Vec<(String, String)>)> {
        let ids: Vec<i32> = {
            let conn = cache.backend.conn.lock().unwrap();
            let mut stmt = conn.prepare("SELECT id FROM books ORDER BY uuid").unwrap();
            let rows = stmt.query_map([], |r| r.get::<_, i32>(0)).unwrap();
            rows.map(Result::unwrap).collect()
        };
        ids.into_iter()
            .map(|id| {
                let get = |f: &str| cache.field_for(id, f).unwrap().unwrap_or_default();
                (get("uuid"), get("title"), get("author_sort"), get("path"), cache.format_file_names(id).unwrap())
            })
            .collect()
    }

    /// Copies only `.calibre-oxide/` into a fresh directory — the
    /// "somebody deleted the database" scenario, or a corrupt one thrown
    /// away.
    fn clone_log_only(from: &std::path::Path, to: &std::path::Path) {
        let src = from.join(crate::constants::LIBRARY_HANDLE_DIR_NAME);
        let dst = to.join(crate::constants::LIBRARY_HANDLE_DIR_NAME);
        std::fs::create_dir_all(&dst).unwrap();
        for sub in ["changes", "snapshots"] {
            let sd = dst.join(sub);
            std::fs::create_dir_all(&sd).unwrap();
            if let Ok(entries) = std::fs::read_dir(src.join(sub)) {
                for entry in entries.flatten() {
                    std::fs::copy(entry.path(), sd.join(entry.file_name())).unwrap();
                }
            }
        }
        // The install id travels too: the rebuilt library is the same
        // peer, and a fresh id would make its own history look like a
        // stranger's.
        if let Ok(id) = std::fs::read(src.join("install-id")) {
            std::fs::write(dst.join("install-id"), id).unwrap();
        }
    }

    /// **The property the whole design rests on.** `metadata.db` is a
    /// cache: lose it and a rebuild from the log reproduces the library.
    /// If this ever fails, the log is not actually authoritative and
    /// calling it so is a lie.
    #[test]
    fn a_library_rebuilds_from_its_log_alone() {
        let original_dir = tempfile::tempdir().unwrap();
        let expected = {
            let cache = Cache::new(original_dir.path()).unwrap();

            let source = original_dir.path().join("scan0001.pdf");
            std::fs::write(&source, b"%PDF-1.4 pretend").unwrap();
            let mut meta = calibre_ebooks::metadata::MetaInformation::default();
            meta.title = "Boiler Manual".to_string();
            meta.authors = vec!["Acme Heating".to_string()];
            let id = cache.add_book(&source, &meta).unwrap();

            cache.set_field(id, "title", "Boiler Manual 1974").unwrap();
            cache.set_field(id, "rating", "8").unwrap();
            cache.rename_format_files(id, "boiler-1974").unwrap();

            let second = original_dir.path().join("other.pdf");
            std::fs::write(&second, b"%PDF-1.4 other").unwrap();
            let mut meta2 = calibre_ebooks::metadata::MetaInformation::default();
            meta2.title = "Something Else".to_string();
            meta2.authors = vec!["Another Author".to_string()];
            let doomed = cache.add_book(&second, &meta2).unwrap();
            // Deleted, so the rebuild has to honour the removal rather
            // than resurrecting it from the earlier add.
            cache.delete_book(doomed).unwrap();

            snapshot(&cache)
        };
        assert_eq!(expected.len(), 1, "one book should survive the delete");

        let rebuilt_dir = tempfile::tempdir().unwrap();
        clone_log_only(original_dir.path(), rebuilt_dir.path());
        assert!(!rebuilt_dir.path().join("metadata.db").exists());

        let rebuilt = Cache::new(rebuilt_dir.path()).unwrap();
        let report = rebuilt.rebuild_from_change_log().unwrap();
        assert_eq!(report.skipped_unknown_book, 0, "every change should have found its book: {report:?}");

        assert_eq!(snapshot(&rebuilt), expected);
    }

    /// A rebuild must be idempotent: running it twice is what happens
    /// when somebody is unsure whether the first one worked.
    #[test]
    fn rebuilding_twice_produces_the_same_library() {
        let original_dir = tempfile::tempdir().unwrap();
        {
            let cache = Cache::new(original_dir.path()).unwrap();
            let source = original_dir.path().join("a.pdf");
            std::fs::write(&source, b"%PDF").unwrap();
            let mut meta = calibre_ebooks::metadata::MetaInformation::default();
            meta.title = "A".to_string();
            meta.authors = vec!["Auth".to_string()];
            let id = cache.add_book(&source, &meta).unwrap();
            cache.set_field(id, "title", "A Better Title").unwrap();
        }

        let rebuilt_dir = tempfile::tempdir().unwrap();
        clone_log_only(original_dir.path(), rebuilt_dir.path());
        let cache = Cache::new(rebuilt_dir.path()).unwrap();

        cache.rebuild_from_change_log().unwrap();
        let once = snapshot(&cache);
        cache.rebuild_from_change_log().unwrap();
        assert_eq!(snapshot(&cache), once);
    }

    /// Without the replay guard, rebuilding would append every change it
    /// was reading, doubling the log each time.
    #[test]
    fn a_replay_does_not_append_to_the_log_it_is_reading() {
        let (_dir, cache) = cache();
        let log = cache.backend.change_log().unwrap();
        log.append(ChangeOp::BookAdded { book: "u1".into() }).unwrap();
        log.append(ChangeOp::FieldSet { book: "u1".into(), field: "title".into(), value: Some("Dune".into()) }).unwrap();
        let before = log.replay().unwrap().len();

        replay_into(&cache, &log).unwrap();

        assert_eq!(log.replay().unwrap().len(), before, "the replay appended to the log");
        // And the flag is off again afterwards, so ordinary writes
        // resume recording.
        assert!(!cache.backend.is_replaying());
    }

    /// The point of carrying schema in the log (#901): a replay has to be
    /// able to rebuild the column *and* the value in it. Without the
    /// definition, a restored `#rating` would have nowhere to go.
    #[test]
    fn replay_rebuilds_a_custom_column_and_then_a_value_in_it() {
        let (_dir, cache) = cache();
        let log = cache.backend.change_log().unwrap();

        // Recorded in the order they happened: the column exists before
        // anything is written to it. The single total order is what makes
        // that hold on replay too.
        let created = log
            .append(ChangeOp::CustomColumnAdded {
                label: "rating".into(),
                name: "My Rating".into(),
                datatype: "int".into(),
                is_multiple: false,
            })
            .unwrap();
        let added = log.append(ChangeOp::BookAdded { book: "uuid-1".into() }).unwrap();

        assert!(apply(&cache, &created).unwrap());
        assert!(apply(&cache, &added).unwrap());

        let column = cache.custom_column_id("rating").unwrap();
        assert!(column.is_some(), "replay did not recreate the column");

        // And it really holds a value, which is the part that would fail
        // if only the value had been logged.
        let book = cache.book_id_for_uuid("uuid-1").unwrap().unwrap();
        cache.set_custom_column_value(book, "rating", "7").unwrap();
        assert_eq!(cache.get_custom_column_value(book, "rating").unwrap().as_deref(), Some("7"));
    }

    #[test]
    fn replaying_a_custom_column_twice_does_not_error_on_the_duplicate_label() {
        let (_dir, cache) = cache();
        let log = cache.backend.change_log().unwrap();
        let created = log
            .append(ChangeOp::CustomColumnAdded { label: "shelf".into(), name: "Shelf".into(), datatype: "text".into(), is_multiple: false })
            .unwrap();

        assert!(apply(&cache, &created).unwrap());
        // `add_custom_column` refuses a duplicate label outright, so
        // replay has to check first -- the same already-there case
        // `BookAdded` handles.
        assert!(apply(&cache, &created).unwrap(), "a second replay must be a no-op, not an error");
    }

    #[test]
    fn replay_removes_a_custom_column_and_tolerates_it_already_being_gone() {
        let (_dir, cache) = cache();
        let log = cache.backend.change_log().unwrap();
        cache.add_custom_column("shelf", "Shelf", "text", false).unwrap();

        let removed = log.append(ChangeOp::CustomColumnRemoved { label: "shelf".into() }).unwrap();
        assert!(apply(&cache, &removed).unwrap());
        assert!(cache.custom_column_id("shelf").unwrap().is_none());
        assert!(apply(&cache, &removed).unwrap(), "already gone is the desired end state");
    }

    #[test]
    fn replay_sets_and_clears_a_library_preference() {
        let (_dir, cache) = cache();
        let log = cache.backend.change_log().unwrap();

        let set = log.append(ChangeOp::PrefSet { key: "sort".into(), value: Some("title".into()) }).unwrap();
        assert!(apply(&cache, &set).unwrap());
        assert_eq!(cache.get_preference("sort").unwrap().as_deref(), Some("title"));

        let cleared = log.append(ChangeOp::PrefSet { key: "sort".into(), value: None }).unwrap();
        assert!(apply(&cache, &cleared).unwrap());
        assert_eq!(cache.get_preference("sort").unwrap(), None);
    }

    /// Library-scoped ops have no book to resolve, so they must not be
    /// filtered out by `replay_into`'s unknown-book guard.
    #[test]
    fn a_library_scoped_change_is_not_skipped_as_an_unknown_book() {
        let (_dir, cache) = cache();
        let log = cache.backend.change_log().unwrap();
        log.append(ChangeOp::PrefSet { key: "sort".into(), value: Some("author".into()) }).unwrap();

        let report = replay_into(&cache, &log).unwrap();
        assert_eq!(report.skipped_unknown_book, 0, "{report:?}");
        assert_eq!(cache.get_preference("sort").unwrap().as_deref(), Some("author"));
    }
}
