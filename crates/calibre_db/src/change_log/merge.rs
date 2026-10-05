//! Merging other machines' changes into a database that is in use (#902,
//! part of #899).
//!
//! The payoff of the whole change-log design: two machines editing one
//! library folder through a file-sync service **converge** instead of
//! corrupting. File sync just delivers files. This is what turns the
//! delivered files into one consistent library.
//!
//! # Why this is not "replay on open"
//!
//! Replaying the whole log in order is already last-writer-wins by
//! construction -- the later stamp is applied later and wins. But a full
//! replay builds the database from *nothing*, and for a database in use
//! that is wrong twice over:
//!
//! - it reassigns every book's local `id`, which is a plain autoincrement
//!   and is what URLs, notes and open windows refer to;
//! - it destroys everything that is not in the log yet (annotations and
//!   reading positions, #967).
//!
//! So a merge applies **only what is new**, on top of what is there. That
//! needs two things a rebuild does not: knowing which changes have been
//! applied, and knowing, per value, how recently it was written.
//!
//! # Per-cell last-writer-wins
//!
//! Applying a peer's change onto a live database has to answer "is this
//! newer than what I already have?" -- because file sync delivers in any
//! order, and a peer's older edit arriving after a newer local one must
//! not win. Each contested value (a [`Cell`]) therefore carries the stamp
//! and origin of the change that last wrote it, and an incoming change
//! applies only if it sorts after that.
//!
//! **Per field, last writer wins.** Union-merging set-valued fields such
//! as tags looks clever and surprises anyone who deliberately removed one.
//! Predictable beats smart.
//!
//! # The merge state lives in `metadata.db`
//!
//! Not in a sidecar file. The clocks describe the state of the database;
//! if they lived anywhere else, deleting `metadata.db` (the recovery step
//! this whole design exists to make safe) would leave clocks claiming
//! changes were applied to a database that no longer has them. In the
//! database, the two can only ever be deleted together.
//!
//! # Applied is a set, not a high-water mark
//!
//! Changes arrive in any order: an origin's seq 7 can be delivered before
//! its seq 6. "Applied through N" would be wrong, so each applied change is
//! recorded by its [`change_key`]. A change that cannot be applied *yet* --
//! its book's `BookAdded` has not arrived -- stays unapplied and is retried
//! on the next merge, rather than being marked done and lost.
//!
//! # Known limitations
//!
//! - **`ItemRenamed` is not last-writer-wins.** It composes rather than
//!   overwrites, so it is applied unconditionally in stamp order. A rename
//!   that arrives *older* than a later local edit naming the old item will
//!   rename that edit's value too. Exact handling needs history.
//! - **Concurrent and sequential edits look the same.** A peer's edit that
//!   replaced a local one is reported as a conflict even when the peer made
//!   it *after* seeing the local one, because an HLC stamp cannot tell the
//!   two apart. Telling them apart needs each change to name the value it
//!   overwrote.

use std::collections::HashSet;

use anyhow::Result;
use rusqlite::OptionalExtension;

use crate::cache::Cache;

use super::apply;
use super::change::{Cell, Change, ChangeOp};
use super::store::{change_key, ChangeLog};

/// Most recent conflicts kept; older ones are pruned so the table cannot
/// grow without bound on a library that syncs for years.
const CONFLICTS_KEPT: i64 = 500;

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS oxide_merge_applied (change_key TEXT PRIMARY KEY);
    CREATE TABLE IF NOT EXISTS oxide_merge_clock (
        scope TEXT NOT NULL, key TEXT NOT NULL, stamp TEXT NOT NULL, origin TEXT NOT NULL,
        PRIMARY KEY (scope, key)
    );
    CREATE TABLE IF NOT EXISTS oxide_merge_meta (key TEXT PRIMARY KEY, val TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS oxide_merge_conflicts (
        id INTEGER PRIMARY KEY AUTOINCREMENT, at_ms INTEGER NOT NULL,
        scope TEXT NOT NULL, key TEXT NOT NULL, winner TEXT NOT NULL,
        local_value TEXT, peer_value TEXT, peer_origin TEXT NOT NULL, peer_stamp TEXT NOT NULL
    );
";

/// Which side of a conflict the library kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Winner {
    /// The local value was newer; the peer's change was not applied.
    Local,
    /// The peer's change was newer and replaced a local edit.
    Peer,
}

impl Winner {
    fn as_str(self) -> &'static str {
        match self {
            Winner::Local => "local",
            Winner::Peer => "peer",
        }
    }
}

/// A resolved disagreement between this machine and a peer, to be shown to
/// the user. A silent merge that reverted an edit is indistinguishable from
/// a bug, so every case where two machines wrote the same value differently
/// is recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// The book's uuid, or `None` for a library-level value.
    pub book: Option<String>,
    /// What was contested: `field:title`, `pref:sort`, ...
    pub what: String,
    pub winner: Winner,
    pub local_value: Option<String>,
    pub peer_value: Option<String>,
    pub peer_origin: String,
}

/// What a merge did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct MergeReport {
    /// Changes applied to the database.
    pub applied: usize,
    /// Changes not applied because the database already holds a newer
    /// value for the same cell.
    pub superseded: usize,
    /// Changes for a book that has not arrived yet. Retried next merge.
    pub pending_unknown_book: usize,
    /// Change files that could not be read yet -- usually a peer's file
    /// seen mid-sync.
    pub unreadable: usize,
    pub conflicts: Vec<Conflict>,
}

fn ensure_schema(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)
}

fn meta_get(conn: &rusqlite::Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT val FROM oxide_merge_meta WHERE key = ?1", [key], |r| r.get(0)).optional()
}

fn meta_set(conn: &rusqlite::Connection, key: &str, val: &str) -> rusqlite::Result<()> {
    conn.execute("INSERT OR REPLACE INTO oxide_merge_meta (key, val) VALUES (?1, ?2)", (key, val))?;
    Ok(())
}

fn clock_get(conn: &rusqlite::Connection, cell: &Cell) -> rusqlite::Result<Option<(String, String)>> {
    conn.query_row("SELECT stamp, origin FROM oxide_merge_clock WHERE scope = ?1 AND key = ?2", (&cell.scope, &cell.key), |r| Ok((r.get(0)?, r.get(1)?))).optional()
}

fn clock_set(conn: &rusqlite::Connection, cell: &Cell, stamp: &str, origin: &str) -> rusqlite::Result<()> {
    conn.execute("INSERT OR REPLACE INTO oxide_merge_clock (scope, key, stamp, origin) VALUES (?1, ?2, ?3, ?4)", (&cell.scope, &cell.key, stamp, origin))?;
    Ok(())
}

fn mark_applied(conn: &rusqlite::Connection, key: &str) -> rusqlite::Result<()> {
    conn.execute("INSERT OR IGNORE INTO oxide_merge_applied (change_key) VALUES (?1)", [key])?;
    Ok(())
}

/// Records a change this machine just made, so a peer's older change for
/// the same value cannot later overwrite it.
///
/// Called from `Cache::record` for every local write. Without it the clock
/// would only ever know about *merged* changes, and a peer's stale edit
/// could beat the user's own newer one on the first merge after it.
pub(crate) fn note_local(cache: &Cache, change: &Change) -> Result<()> {
    let conn = cache.backend.conn.lock().unwrap();
    ensure_schema(&conn)?;
    mark_applied(&conn, &change_key(change))?;
    if let Some(cell) = change.op.cell() {
        let stamp = change.hlc.file_prefix();
        // Only ever moves forward, so replaying an old local change cannot
        // wind the clock back.
        let newer = clock_get(&conn, &cell)?.is_none_or(|(s, o)| (s, o) < (stamp.clone(), change.origin.clone()));
        if newer {
            clock_set(&conn, &cell, &stamp, &change.origin)?;
        }
    }
    Ok(())
}

/// The current value of what `op` writes, for spotting a disagreement.
fn current_value(cache: &Cache, op: &ChangeOp) -> Option<String> {
    match op {
        ChangeOp::FieldSet { book, field, .. } => {
            let id = cache.book_id_for_uuid(book).ok()??;
            match field.strip_prefix('#') {
                Some(label) => cache.get_custom_column_value(id, label).ok()?,
                None => cache.field_for(id, field).ok()?,
            }
        }
        ChangeOp::PrefSet { key, .. } => cache.get_preference(key).ok()?,
        _ => None,
    }
}

/// The value `op` would write, for the same comparison.
fn incoming_value(op: &ChangeOp) -> Option<String> {
    match op {
        ChangeOp::FieldSet { value, .. } | ChangeOp::PrefSet { value, .. } => value.clone(),
        _ => None,
    }
}

fn record_conflict(cache: &Cache, conflict: &Conflict, peer_stamp: &str) {
    let conn = cache.backend.conn.lock().unwrap();
    let at_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
    let scope = conflict.book.clone().unwrap_or_default();
    let _ = conn.execute(
        "INSERT INTO oxide_merge_conflicts (at_ms, scope, key, winner, local_value, peer_value, peer_origin, peer_stamp) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        (at_ms, &scope, &conflict.what, conflict.winner.as_str(), &conflict.local_value, &conflict.peer_value, &conflict.peer_origin, peer_stamp),
    );
    let _ = conn.execute("DELETE FROM oxide_merge_conflicts WHERE id NOT IN (SELECT id FROM oxide_merge_conflicts ORDER BY id DESC LIMIT ?1)", [CONFLICTS_KEPT]);
}


/// The merge's own state writes, buffered.
///
/// Recording that a change was applied, and the stamp of the value it
/// wrote, was two autocommitted `INSERT`s per change -- each its own
/// transaction and its own fsync. Rebuilding a 20,000-change log spent most
/// of its time there. Buffered and flushed in one transaction per
/// [`FLUSH_EVERY`] changes instead.
///
/// Reads go through [`Pending::clock`], which consults the buffer first, so
/// a change later in the same batch sees the clock an earlier one set.
///
/// A crash between applying a change and flushing leaves it unrecorded, so
/// the next merge applies it again. That is safe: every op sets a value
/// rather than adjusting one, and the clock is compared before applying.
const FLUSH_EVERY: usize = 500;

#[derive(Default)]
struct Pending {
    applied: Vec<String>,
    clocks: std::collections::HashMap<Cell, (String, String)>,
}

impl Pending {
    fn clock(&self, conn: &rusqlite::Connection, cell: &Cell) -> rusqlite::Result<Option<(String, String)>> {
        match self.clocks.get(cell) {
            Some(found) => Ok(Some(found.clone())),
            None => clock_get(conn, cell),
        }
    }

    fn len(&self) -> usize {
        self.applied.len()
    }

    fn flush(&mut self, conn: &mut rusqlite::Connection) -> rusqlite::Result<()> {
        if self.applied.is_empty() && self.clocks.is_empty() {
            return Ok(());
        }
        let tx = conn.transaction()?;
        for key in self.applied.drain(..) {
            mark_applied(&tx, &key)?;
        }
        for (cell, (stamp, origin)) in self.clocks.drain() {
            clock_set(&tx, &cell, &stamp, &origin)?;
        }
        tx.commit()
    }
}

fn flush_if_due(cache: &Cache, pending: &mut Pending) -> rusqlite::Result<()> {
    if pending.len() >= FLUSH_EVERY {
        let mut conn = cache.backend.conn.lock().unwrap();
        pending.flush(&mut conn)?;
    }
    Ok(())
}

/// Applies every change on disk that this database has not seen.
///
/// Safe to call whenever: with nothing new it is one directory listing. On
/// a database with no merge state at all it behaves as a full rebuild.
pub fn merge_pending(cache: &Cache) -> Result<MergeReport> {
    let mut report = MergeReport::default();
    // Look before touching: a library that has never had a log should not
    // grow one because something opened it.
    if !ChangeLog::has_entries(&cache.backend.library_path) {
        return Ok(report);
    }
    let log = cache.backend.change_log()?;
    let local_origin = log.origin().as_str().to_string();

    {
        let conn = cache.backend.conn.lock().unwrap();
        ensure_schema(&conn)?;
    }

    let applied: HashSet<String> = {
        let conn = cache.backend.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT change_key FROM oxide_merge_applied")?;
        let keys = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<HashSet<_>>>()?;
        keys
    };

    let mut found = log.unapplied(&applied)?;
    report.unreadable = found.unreadable.len();

    // A snapshot stands in for changes whose files compaction deleted. Only
    // read it when it is a different one from last time.
    let seen_snapshot = {
        let conn = cache.backend.conn.lock().unwrap();
        meta_get(&conn, "snapshot")?
    };
    let newest_snapshot = log.newest_snapshot_name();
    let reading_snapshot = newest_snapshot.is_some() && newest_snapshot != seen_snapshot;
    if reading_snapshot {
        let known: HashSet<String> = found.changes.iter().map(change_key).collect();
        for change in log.snapshot_changes()? {
            let key = change_key(&change);
            if !applied.contains(&key) && !known.contains(&key) {
                found.changes.push(change);
            }
        }
        found.changes.sort_by(super::change::total_order);
    }

    // First time this database has been merged into. If it already holds
    // books, then every change *this* install made was applied by the write
    // that made it -- re-applying them would be pointless, so they are
    // adopted: marked applied, and their stamps recorded so a peer's older
    // edit cannot beat them.
    let first_time = {
        let conn = cache.backend.conn.lock().unwrap();
        meta_get(&conn, "initialised")?.is_none()
    };
    let has_books: bool = {
        let conn = cache.backend.conn.lock().unwrap();
        conn.query_row("SELECT EXISTS(SELECT 1 FROM books)", [], |r| r.get(0))?
    };
    let adopt_own = first_time && has_books;

    // Adoption is its own pass, finished before any peer change is looked
    // at. Done inline, in stamp order, a peer's *old* change is reached
    // before this install's own newer ones have recorded their clocks: it
    // finds no clock, applies, and overwrites the local value. (Found by a
    // test, not by reading.)
    if adopt_own {
        let conn = cache.backend.conn.lock().unwrap();
        for change in found.changes.iter().filter(|c| c.origin == local_origin) {
            mark_applied(&conn, &change_key(change))?;
            if let Some(cell) = change.op.cell() {
                let stamp = change.hlc.file_prefix();
                if clock_get(&conn, &cell)?.is_none_or(|(s, o)| (s, o) < (stamp.clone(), change.origin.clone())) {
                    clock_set(&conn, &cell, &stamp, &change.origin)?;
                }
            }
        }
        drop(conn);
        found.changes.retain(|c| c.origin != local_origin);
    }

    let _guard = cache.backend.begin_replay();
    let mut pending = Pending::default();
    for change in &found.changes {
        log.observe(change.hlc);
        let key = change_key(change);
        let stamp = change.hlc.file_prefix();

        let Some(cell) = change.op.cell() else {
            // Composes rather than overwrites; applied unconditionally.
            if apply::apply(cache, change)? {
                pending.applied.push(key);
                report.applied += 1;
            } else {
                report.pending_unknown_book += 1;
            }
            flush_if_due(cache, &mut pending)?;
            continue;
        };

        let existing = {
            let conn = cache.backend.conn.lock().unwrap();
            pending.clock(&conn, &cell)?
        };
        let incoming = (stamp.clone(), change.origin.clone());

        if let Some(existing) = &existing {
            if *existing >= incoming {
                // The database already holds this or something newer.
                if *existing > incoming && existing.1 == local_origin && change.origin != local_origin {
                    // A peer's concurrent edit lost to one made here.
                    let local_value = current_value(cache, &change.op);
                    let peer_value = incoming_value(&change.op);
                    if local_value != peer_value {
                        let conflict = Conflict {
                            book: (!cell.scope.is_empty()).then(|| cell.scope.clone()),
                            what: cell.key.clone(),
                            winner: Winner::Local,
                            local_value,
                            peer_value,
                            peer_origin: change.origin.clone(),
                        };
                        record_conflict(cache, &conflict, &stamp);
                        report.conflicts.push(conflict);
                    }
                }
                pending.applied.push(key);
                report.superseded += 1;
                flush_if_due(cache, &mut pending)?;
                continue;
            }
        }

        // Read the value being replaced only when a conflict is possible:
        // the previous writer was *this* install and the new one is not.
        // `field_for` reloads the in-memory field store after every write,
        // so reading it before each apply made a rebuild cost a full store
        // reload per change -- the bulk of 16 seconds for 20,000 of them.
        let may_conflict = change.origin != local_origin && existing.as_ref().is_some_and(|(_, o)| *o == local_origin);
        let before = if may_conflict { current_value(cache, &change.op) } else { None };
        if apply::apply(cache, change)? {
            if let Some((_, previous_origin)) = &existing {
                let peer_value = incoming_value(&change.op);
                if *previous_origin == local_origin && change.origin != local_origin && before != peer_value {
                    // A peer's edit replaced one made here.
                    let conflict = Conflict {
                        book: (!cell.scope.is_empty()).then(|| cell.scope.clone()),
                        what: cell.key.clone(),
                        winner: Winner::Peer,
                        local_value: before,
                        peer_value,
                        peer_origin: change.origin.clone(),
                    };
                    record_conflict(cache, &conflict, &stamp);
                    report.conflicts.push(conflict);
                }
            }
            pending.clocks.insert(cell, incoming);
            pending.applied.push(key);
            report.applied += 1;
        } else {
            // Its book has not arrived yet. Left unapplied, so the next
            // merge tries again -- marking it done would lose it.
            report.pending_unknown_book += 1;
        }
        flush_if_due(cache, &mut pending)?;
    }
    {
        let mut conn = cache.backend.conn.lock().unwrap();
        pending.flush(&mut conn)?;
    }

    let conn = cache.backend.conn.lock().unwrap();
    meta_set(&conn, "initialised", "1")?;
    if reading_snapshot {
        if let Some(name) = &newest_snapshot {
            meta_set(&conn, "snapshot", name)?;
        }
    }
    Ok(report)
}

/// The conflicts recorded by past merges, newest first.
pub fn recent_conflicts(cache: &Cache, limit: usize) -> Result<Vec<Conflict>> {
    let conn = cache.backend.conn.lock().unwrap();
    ensure_schema(&conn)?;
    let mut stmt = conn.prepare("SELECT scope, key, winner, local_value, peer_value, peer_origin FROM oxide_merge_conflicts ORDER BY id DESC LIMIT ?1")?;
    let rows = stmt.query_map([limit as i64], |r| {
        let scope: String = r.get(0)?;
        let winner: String = r.get(2)?;
        Ok(Conflict {
            book: (!scope.is_empty()).then_some(scope),
            what: r.get(1)?,
            winner: if winner == "peer" { Winner::Peer } else { Winner::Local },
            local_value: r.get(3)?,
            peer_value: r.get(4)?,
            peer_origin: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change_log::hlc::Hlc;
    use calibre_ebooks::metadata::MetaInformation;
    use std::path::Path;

    /// A peer with its own install id, as a second machine would have.
    const PEER: &str = "aaaaaaaaaaaaaaaa";
    const OTHER_PEER: &str = "bbbbbbbbbbbbbbbb";

    fn library() -> (tempfile::TempDir, Cache) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        (dir, cache)
    }

    fn add_book(dir: &Path, cache: &Cache, title: &str) -> i32 {
        let source = dir.join(format!("{title}.txt"));
        std::fs::write(&source, format!("{title} body")).unwrap();
        let mut meta = MetaInformation::default();
        meta.title = title.to_string();
        meta.authors = vec!["An Author".to_string()];
        cache.add_book(&source, &meta).unwrap()
    }

    /// Writes a change a peer "made", with a stamp of the test's choosing,
    /// straight into a library's log directory -- which is all file sync
    /// does.
    fn deliver(dir: &Path, origin: &str, seq: u64, wall_ms: u64, op: ChangeOp) {
        let change = Change::new(Hlc { wall_ms, counter: 0 }, origin.to_string(), seq, None, op);
        let changes = dir.join(crate::constants::LIBRARY_HANDLE_DIR_NAME).join("changes");
        std::fs::create_dir_all(&changes).unwrap();
        let name = change.file_name(&format!("{seq:08x}"));
        std::fs::write(changes.join(name), serde_json::to_vec(&change).unwrap()).unwrap();
    }

    fn set_title(book: &str, title: &str) -> ChangeOp {
        ChangeOp::FieldSet { book: book.into(), field: "title".into(), value: Some(title.into()) }
    }

    fn title_of(cache: &Cache, uuid: &str) -> Option<String> {
        let id = cache.book_id_for_uuid(uuid).unwrap()?;
        cache.field_for(id, "title").unwrap()
    }

    /// Far in the future, so it beats anything stamped by the real clock.
    const FUTURE: u64 = 4_000_000_000_000;
    /// 1970, so it loses to anything stamped by the real clock.
    const ANCIENT: u64 = 1_000;

    // -- the epic's headline: delete the database and it rebuilds ----------

    /// #899's central claim, with no explicit rebuild call: open a library
    /// whose `metadata.db` is gone and the books are back.
    #[test]
    fn opening_a_library_with_no_database_rebuilds_it_from_the_log() {
        let (original_dir, original) = library();
        let id = add_book(original_dir.path(), &original, "Recovered Book");
        original.set_field(id, "rating", "8").unwrap();
        let uuid = original.book_uuid(id).unwrap().unwrap();

        // Only the log survives: copy it to a directory with no database.
        let recovered_dir = tempfile::tempdir().unwrap();
        let from = original_dir.path().join(crate::constants::LIBRARY_HANDLE_DIR_NAME).join("changes");
        let to = recovered_dir.path().join(crate::constants::LIBRARY_HANDLE_DIR_NAME).join("changes");
        std::fs::create_dir_all(&to).unwrap();
        for entry in std::fs::read_dir(from).unwrap().flatten() {
            std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
        }
        assert!(!recovered_dir.path().join("metadata.db").exists());

        let recovered = Cache::new(recovered_dir.path()).unwrap();

        assert_eq!(title_of(&recovered, &uuid).as_deref(), Some("Recovered Book"));
        let recovered_id = recovered.book_id_for_uuid(&uuid).unwrap().unwrap();
        assert_eq!(recovered.field_for(recovered_id, "rating").unwrap().as_deref(), Some("8"));
    }

    // -- a merge applies only what is new, on top of what is there ---------

    /// The reason this is not a full replay: ids are local autoincrements
    /// and URLs, notes and open windows refer to them.
    #[test]
    fn merging_a_peers_book_leaves_existing_book_ids_alone() {
        let (dir, cache) = library();
        let first = add_book(dir.path(), &cache, "First");
        let second = add_book(dir.path(), &cache, "Second");

        deliver(dir.path(), PEER, 1, FUTURE, ChangeOp::BookAdded { book: "peer-book".into() });
        deliver(dir.path(), PEER, 2, FUTURE + 1, set_title("peer-book", "From A Peer"));
        let report = cache.merge_pending_changes().unwrap();

        assert_eq!(report.applied, 2, "{report:?}");
        assert_eq!(cache.field_for(first, "title").unwrap().as_deref(), Some("First"));
        assert_eq!(cache.field_for(second, "title").unwrap().as_deref(), Some("Second"));
        assert_eq!(title_of(&cache, "peer-book").as_deref(), Some("From A Peer"));
        let peer_id = cache.book_id_for_uuid("peer-book").unwrap().unwrap();
        assert!(peer_id > second, "the peer's book should be appended, not renumber the others");
    }

    #[test]
    fn merging_twice_changes_nothing_the_second_time() {
        let (dir, cache) = library();
        add_book(dir.path(), &cache, "Existing");
        deliver(dir.path(), PEER, 1, FUTURE, ChangeOp::BookAdded { book: "peer-book".into() });
        deliver(dir.path(), PEER, 2, FUTURE + 1, set_title("peer-book", "Once"));

        let first = cache.merge_pending_changes().unwrap();
        let second = cache.merge_pending_changes().unwrap();

        assert_eq!(first.applied, 2);
        assert_eq!(second, MergeReport::default(), "nothing is new the second time");
    }

    // -- per-field last writer wins by stamp --------------------------------

    #[test]
    fn a_newer_peer_edit_replaces_a_local_one_and_is_reported() {
        let (dir, cache) = library();
        let id = add_book(dir.path(), &cache, "Mine");
        cache.set_field(id, "title", "Local Edit").unwrap();
        let uuid = cache.book_uuid(id).unwrap().unwrap();

        deliver(dir.path(), PEER, 1, FUTURE, set_title(&uuid, "Peer Edit"));
        let report = cache.merge_pending_changes().unwrap();

        assert_eq!(title_of(&cache, &uuid).as_deref(), Some("Peer Edit"));
        assert_eq!(report.conflicts.len(), 1, "{report:?}");
        let conflict = &report.conflicts[0];
        assert_eq!(conflict.winner, Winner::Peer);
        assert_eq!(conflict.local_value.as_deref(), Some("Local Edit"));
        assert_eq!(conflict.peer_value.as_deref(), Some("Peer Edit"));
        assert_eq!(conflict.peer_origin, PEER);
    }

    /// File sync delivers in any order. A peer's *older* edit arriving
    /// after a newer local one must not win.
    #[test]
    fn an_older_peer_edit_arriving_late_does_not_overwrite_a_newer_local_one() {
        let (dir, cache) = library();
        let id = add_book(dir.path(), &cache, "Mine");
        cache.set_field(id, "title", "Newer Local Edit").unwrap();
        let uuid = cache.book_uuid(id).unwrap().unwrap();

        deliver(dir.path(), PEER, 1, ANCIENT, set_title(&uuid, "Older Peer Edit"));
        let report = cache.merge_pending_changes().unwrap();

        assert_eq!(title_of(&cache, &uuid).as_deref(), Some("Newer Local Edit"), "an old change won");
        assert_eq!(report.applied, 0);
        assert_eq!(report.superseded, 1);
        assert_eq!(report.conflicts.len(), 1);
        assert_eq!(report.conflicts[0].winner, Winner::Local);
    }

    /// Two machines must end in the same state whichever order the files
    /// reached each of them.
    #[test]
    fn peers_converge_whatever_order_changes_arrive_in() {
        let book = ChangeOp::BookAdded { book: "shared".into() };
        let early = set_title("shared", "Early");
        let late = set_title("shared", "Late");

        let (dir_a, a) = library();
        deliver(dir_a.path(), PEER, 1, 5_000_000_000_000, book.clone());
        deliver(dir_a.path(), PEER, 2, 5_000_000_000_100, early.clone());
        deliver(dir_a.path(), OTHER_PEER, 1, 5_000_000_000_200, late.clone());
        a.merge_pending_changes().unwrap();

        // Same changes, but the newest is delivered *first* and the rest in
        // a second merge.
        let (dir_b, b) = library();
        deliver(dir_b.path(), PEER, 1, 5_000_000_000_000, book);
        deliver(dir_b.path(), OTHER_PEER, 1, 5_000_000_000_200, late);
        b.merge_pending_changes().unwrap();
        deliver(dir_b.path(), PEER, 2, 5_000_000_000_100, early);
        b.merge_pending_changes().unwrap();

        assert_eq!(title_of(&a, "shared").as_deref(), Some("Late"));
        assert_eq!(title_of(&b, "shared"), title_of(&a, "shared"), "arrival order changed the outcome");
    }

    /// A format removed at stamp 100 must not be resurrected by a peer's
    /// "format added" at stamp 50 that merely arrived later.
    #[test]
    fn an_older_format_add_does_not_resurrect_a_newer_removal() {
        let (dir, cache) = library();
        deliver(dir.path(), PEER, 1, FUTURE, ChangeOp::BookAdded { book: "b".into() });
        deliver(dir.path(), PEER, 2, FUTURE + 100, ChangeOp::FormatRemoved { book: "b".into(), format: "EPUB".into() });
        cache.merge_pending_changes().unwrap();

        deliver(dir.path(), OTHER_PEER, 1, FUTURE + 50, ChangeOp::FormatSet { book: "b".into(), format: "EPUB".into(), name: "n".into(), size: 1, hash: None });
        let report = cache.merge_pending_changes().unwrap();

        let id = cache.book_id_for_uuid("b").unwrap().unwrap();
        assert!(cache.format_file_names(id).unwrap().is_empty(), "a removed format came back");
        assert_eq!(report.superseded, 1);
    }

    // -- changes that cannot be applied yet --------------------------------

    /// The peer's `BookAdded` has not synced yet. Skipped, not invented --
    /// and crucially *not marked done*, or it would be lost.
    #[test]
    fn a_change_for_a_book_that_has_not_arrived_waits_and_is_applied_later() {
        let (dir, cache) = library();
        deliver(dir.path(), PEER, 2, FUTURE + 1, set_title("late-book", "Arrived Out Of Order"));

        let first = cache.merge_pending_changes().unwrap();
        assert_eq!(first.pending_unknown_book, 1);
        assert_eq!(first.applied, 0);
        assert_eq!(title_of(&cache, "late-book"), None, "a book was invented from a change that names an unknown one");

        deliver(dir.path(), PEER, 1, FUTURE, ChangeOp::BookAdded { book: "late-book".into() });
        let second = cache.merge_pending_changes().unwrap();

        assert_eq!(second.applied, 2, "the waiting change should have been retried: {second:?}");
        assert_eq!(title_of(&cache, "late-book").as_deref(), Some("Arrived Out Of Order"));
    }

    /// A peer's file seen mid-sync parses next time.
    #[test]
    fn a_half_written_change_file_is_retried_not_dropped() {
        let (dir, cache) = library();
        let changes = dir.path().join(crate::constants::LIBRARY_HANDLE_DIR_NAME).join("changes");
        std::fs::create_dir_all(&changes).unwrap();
        let change = Change::new(Hlc { wall_ms: FUTURE, counter: 0 }, PEER.to_string(), 1, None, ChangeOp::BookAdded { book: "syncing".into() });
        let path = changes.join(change.file_name("00000001"));
        let full = serde_json::to_vec(&change).unwrap();
        std::fs::write(&path, &full[..full.len() / 2]).unwrap();

        let first = cache.merge_pending_changes().unwrap();
        assert_eq!(first.unreadable, 1);
        assert_eq!(cache.book_id_for_uuid("syncing").unwrap(), None);

        std::fs::write(&path, &full).unwrap();
        let second = cache.merge_pending_changes().unwrap();
        assert_eq!(second.applied, 1, "{second:?}");
        assert!(cache.book_id_for_uuid("syncing").unwrap().is_some());
    }

    #[test]
    fn a_cloud_sync_conflict_copy_is_not_replayed() {
        let (dir, cache) = library();
        deliver(dir.path(), PEER, 1, FUTURE, ChangeOp::BookAdded { book: "once".into() });
        let changes = dir.path().join(crate::constants::LIBRARY_HANDLE_DIR_NAME).join("changes");
        let original = std::fs::read_dir(&changes).unwrap().flatten().find(|e| e.file_name().to_string_lossy().contains(PEER)).unwrap();
        let copy_name = original.file_name().to_string_lossy().replace(".json", " (conflicted copy 2026-01-01).json");
        std::fs::copy(original.path(), changes.join(copy_name)).unwrap();

        let report = cache.merge_pending_changes().unwrap();
        assert_eq!(report.applied, 1, "the conflict copy was replayed too: {report:?}");
    }

    // -- the clock ----------------------------------------------------------

    /// A peer's stamp from the future must not keep winning against edits
    /// made after it was merged.
    #[test]
    fn a_local_edit_made_after_merging_a_future_stamp_sorts_after_it() {
        let (dir, cache) = library();
        let id = add_book(dir.path(), &cache, "Mine");
        let uuid = cache.book_uuid(id).unwrap().unwrap();
        deliver(dir.path(), PEER, 1, FUTURE, set_title(&uuid, "Peer, From The Future"));
        cache.merge_pending_changes().unwrap();

        cache.set_field(id, "title", "Edited After Seeing It").unwrap();

        let log = cache.backend.change_log().unwrap();
        let mine = log.replay().unwrap().changes().filter(|c| c.origin == log.origin().as_str()).map(|c| c.hlc).max().unwrap();
        assert!(mine.wall_ms >= FUTURE, "the clock was not pulled past the peer's stamp: {mine:?}");
    }

    /// The first merge into a database that already has books adopts this
    /// install's own history rather than re-applying it, *and* records its
    /// stamps -- so a peer's older edit still loses to it afterwards.
    #[test]
    fn a_database_with_no_merge_state_adopts_its_own_history_and_still_protects_it() {
        let (dir, cache) = library();
        let id = add_book(dir.path(), &cache, "Legacy");
        cache.set_field(id, "title", "Legacy Edit").unwrap();
        let uuid = cache.book_uuid(id).unwrap().unwrap();
        // A library from before merge state existed.
        {
            let conn = cache.backend.conn.lock().unwrap();
            conn.execute_batch("DROP TABLE oxide_merge_applied; DROP TABLE oxide_merge_clock; DROP TABLE oxide_merge_meta; DROP TABLE oxide_merge_conflicts;").unwrap();
        }
        deliver(dir.path(), PEER, 1, ANCIENT, set_title(&uuid, "Ancient Peer Edit"));

        let report = cache.merge_pending_changes().unwrap();

        assert_eq!(title_of(&cache, &uuid).as_deref(), Some("Legacy Edit"));
        assert_eq!(report.applied, 0, "own history was re-applied: {report:?}");
        assert_eq!(report.superseded, 1, "the peer's old edit should have lost: {report:?}");
    }

    #[test]
    fn conflicts_are_kept_so_they_can_be_shown_later() {
        let (dir, cache) = library();
        let id = add_book(dir.path(), &cache, "Mine");
        cache.set_field(id, "title", "Local").unwrap();
        let uuid = cache.book_uuid(id).unwrap().unwrap();
        deliver(dir.path(), PEER, 1, FUTURE, set_title(&uuid, "Peer"));
        cache.merge_pending_changes().unwrap();

        let kept = cache.recent_merge_conflicts(10).unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].what, "field:title");
        assert_eq!(kept[0].peer_value.as_deref(), Some("Peer"));
    }

    // -- the thread-local replay guard --------------------------------------

    /// Merging must not stop a *different thread's* edit being logged. The
    /// guard used to be a flag on the backend, so it did.
    #[test]
    fn a_merge_in_progress_does_not_suppress_logging_on_other_threads() {
        let (dir, cache) = library();
        let id = add_book(dir.path(), &cache, "Concurrent");
        let cache = std::sync::Arc::new(cache);

        let _merging = cache.backend.begin_replay();
        assert!(cache.backend.is_replaying(), "this thread is replaying");

        let other = {
            let cache = cache.clone();
            std::thread::spawn(move || {
                assert!(!cache.backend.is_replaying(), "another thread was suppressed by a replay it is not part of");
                cache.set_field(id, "title", "Edited While Merging").unwrap();
            })
        };
        other.join().unwrap();

        let log = cache.backend.change_log().unwrap();
        let logged = log.replay().unwrap().changes().any(|c| matches!(&c.op, ChangeOp::FieldSet { field, value: Some(v), .. } if field == "title" && v == "Edited While Merging"));
        assert!(logged, "an edit made while a merge ran on another thread never reached the log");
    }

    // -- two real machines ---------------------------------------------------

    /// What a file-sync service does: every change file either side has and
    /// the other lacks is copied across. The install id is *not* copied --
    /// each machine is its own peer.
    fn sync(a: &Path, b: &Path) {
        let changes = |root: &Path| root.join(crate::constants::LIBRARY_HANDLE_DIR_NAME).join("changes");
        std::fs::create_dir_all(changes(a)).unwrap();
        std::fs::create_dir_all(changes(b)).unwrap();
        for (from, to) in [(changes(a), changes(b)), (changes(b), changes(a))] {
            for entry in std::fs::read_dir(&from).unwrap().flatten() {
                let target = to.join(entry.file_name());
                if !target.exists() {
                    std::fs::copy(entry.path(), target).unwrap();
                }
            }
        }
    }

    /// The scenario the whole design exists for: two machines edit the same
    /// library folder, file sync moves the files, and both end up the same.
    #[test]
    fn two_machines_editing_the_same_book_converge_and_neither_loses_a_field() {
        let (dir_a, a) = library();
        let id_a = add_book(dir_a.path(), &a, "Shared Book");
        let uuid = a.book_uuid(id_a).unwrap().unwrap();

        // Machine B starts from A's log alone -- no database copied.
        let dir_b = tempfile::tempdir().unwrap();
        sync(dir_a.path(), dir_b.path());
        let b = Cache::new(dir_b.path()).unwrap();
        let id_b = b.book_id_for_uuid(&uuid).unwrap().expect("B should have rebuilt A's book from the log");
        assert_ne!(a.backend.change_log().unwrap().origin(), b.backend.change_log().unwrap().origin(), "they must be different peers");

        // Both edit while out of contact. Different fields, and one field both.
        a.set_field(id_a, "rating", "8").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        b.set_field(id_b, "publisher", "B's Press").unwrap();
        a.set_field(id_a, "title", "A's Title").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        b.set_field(id_b, "title", "B's Title").unwrap(); // later, so it should win

        sync(dir_a.path(), dir_b.path());
        let report_a = a.merge_pending_changes().unwrap();
        let report_b = b.merge_pending_changes().unwrap();

        let read = |c: &Cache, id: i32, f: &str| c.field_for(id, f).unwrap();
        // Different fields: both edits survive.
        assert_eq!(read(&a, id_a, "rating").as_deref(), Some("8"));
        assert_eq!(read(&b, id_b, "rating").as_deref(), Some("8"), "B lost A's rating");
        assert_eq!(read(&a, id_a, "publisher").as_deref(), Some("B's Press"), "A lost B's publisher");
        // Same field: the later write wins, on both.
        assert_eq!(read(&a, id_a, "title").as_deref(), Some("B's Title"));
        assert_eq!(read(&b, id_b, "title").as_deref(), Some("B's Title"));

        // A was overruled, so A is told; B kept its own value, so B is told
        // a peer's edit lost.
        assert_eq!(report_a.conflicts.iter().filter(|c| c.what == "field:title").count(), 1, "{report_a:?}");
        assert_eq!(report_b.conflicts.iter().filter(|c| c.what == "field:title").count(), 1, "{report_b:?}");

        // And a second round with nothing new is a no-op on both.
        sync(dir_a.path(), dir_b.path());
        assert_eq!(a.merge_pending_changes().unwrap(), MergeReport::default());
        assert_eq!(b.merge_pending_changes().unwrap(), MergeReport::default());
    }

    /// Opening a library to read it must not leave a change log behind. The
    /// merge looks before it touches: `ChangeLog::open` creates directories
    /// and an install id.
    #[test]
    fn opening_a_library_that_has_never_had_a_log_does_not_create_one() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();

        let report = cache.merge_pending_changes().unwrap();

        assert_eq!(report, MergeReport::default());
        assert!(!dir.path().join(crate::constants::LIBRARY_HANDLE_DIR_NAME).join("changes").exists(), "a read-only open created a change log");
    }
}

