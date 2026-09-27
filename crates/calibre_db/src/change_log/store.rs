//! The on-disk change log: `.calibre-oxide/changes/`, plus the
//! snapshots that keep it from growing forever.
//!
//! # Why there is no single hash chain
//!
//! `library_handle.rs`'s write-ahead journal chains every entry to the
//! previous one, so a missing or edited entry breaks the chain and is
//! reported as corruption. That works because it has exactly one
//! writer.
//!
//! This log has as many writers as the user has machines, and they
//! never talk to each other — the log is merged by file sync. A single
//! chain would require each writer to know the other's most recent
//! entry before writing its own, which is precisely what they cannot
//! do. Two peers extending one chain produce two entries claiming the
//! same predecessor, and the chain is broken by normal use.
//!
//! So the log is chained **per origin**: every install has its own
//! sequence and its own chain, and the merged log is several chains
//! side by side. That keeps what the chain is for — a deleted or
//! tampered entry is detectable, because it leaves a gap or a mismatch
//! in *that origin's* sequence — without requiring coordination that
//! does not exist. It is the same reason git branches rather than
//! demanding a global commit order.
//!
//! # Why compaction has a horizon
//!
//! The log grows by one small file per edit, so it needs collapsing.
//! The naive version — write a snapshot of the current state, delete
//! every change it covers — destroys data in exactly the setup this
//! design exists for: a peer that has been offline for a month has
//! changes worth keeping, and deleting them here means they are gone
//! from the only copy that would have delivered them. So compaction
//! only touches changes older than a horizon, and the horizon has to be
//! longer than the longest time a machine might be away.
//!
//! # Two logs, deliberately separate
//!
//! `journal/` guards *physical file writes* for crash atomicity, and is
//! consumed and pruned during recovery — its entries are worthless once
//! the write they describe has landed. `changes/` is *durable metadata
//! history* and is discarded only by compaction. They look alike and
//! have opposite lifetimes; merging them would give one of them the
//! wrong one.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::constants::LIBRARY_HANDLE_DIR_NAME;

use super::change::{total_order, Change, ChangeOp, ChangeParseError};
use super::hlc::{Hlc, HlcClock};

/// How long a change is kept before compaction may collapse it.
///
/// Sized for the failure it exists to prevent: a laptop that has not
/// been opened in a month still holding the only copy of its own edits.
/// Shorter would be tidier and would lose data.
pub const DEFAULT_RETENTION: Duration = Duration::from_secs(60 * 60 * 24 * 60);

/// Identifies one installation of the app. Persisted, because it is
/// half of every change's identity and a new one each run would make
/// every restart look like a new peer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallId(String);

impl InstallId {
    /// Reads this install's id, creating it on first use.
    ///
    /// Lives inside the library rather than in the user's config dir on
    /// purpose: the id has to be stable *for this library*, and a
    /// library carried to another machine on a USB stick and opened
    /// there must not claim to be the same peer.
    pub fn load_or_create(dir: &Path) -> Result<Self> {
        let path = dir.join("install-id");
        if let Ok(existing) = fs::read_to_string(&path) {
            let trimmed = existing.trim();
            if !trimmed.is_empty() {
                return Ok(InstallId(trimmed.to_string()));
            }
        }
        // 16 hex characters: short enough to sit in every filename,
        // wide enough that two installs colliding is not a thing that
        // happens.
        let id = uuid::Uuid::new_v4().simple().to_string()[..16].to_string();
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        fs::write(&path, &id).with_context(|| format!("writing {}", path.display()))?;
        Ok(InstallId(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Where one origin's chain stood when a snapshot was taken.
///
/// The hash matters as much as the sequence: without it, verification
/// could not tell a legitimately compacted gap from a deleted entry,
/// because the first surviving entry's `prev` would point at something
/// no longer on disk. Same role as `library_handle.rs`'s
/// `JournalCheckpoint::boundary_hash`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Watermark {
    pub seq: u64,
    pub hash: String,
}

/// A snapshot is just a collapsed change list.
///
/// Deliberately not a materialised copy of the database: that would be
/// a second schema to keep in step, and replaying it would need its own
/// code path. Collapsing to "the minimum set of changes producing this
/// state" means snapshot and tail replay through exactly the same
/// apply logic, and the file stays inspectable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotHeader {
    /// Per origin, the last sequence this snapshot accounts for.
    pub watermarks: BTreeMap<String, Watermark>,
    pub taken_at: Hlc,
}

/// Everything needed to rebuild: a collapsed prefix, then the entries
/// after it.
#[derive(Debug, Clone, Default)]
pub struct Replay {
    pub snapshot: Vec<Change>,
    pub tail: Vec<Change>,
}

impl Replay {
    /// Every change to apply, in order.
    pub fn changes(&self) -> impl Iterator<Item = &Change> {
        self.snapshot.iter().chain(self.tail.iter())
    }

    pub fn len(&self) -> usize {
        self.snapshot.len() + self.tail.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Debug, Default)]
pub struct CompactionReport {
    /// Entries collapsed away because a later change supersedes them.
    pub dropped: usize,
    /// Entries kept in the new snapshot.
    pub retained: usize,
    /// Entries left alone because they are inside the retention
    /// horizon.
    pub within_horizon: usize,
}

/// Problems found by [`ChangeLog::verify`]. Reported rather than
/// returned as an error so one damaged entry does not hide the rest.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct VerifyReport {
    /// Files that could not be read or whose hash did not match.
    pub unreadable: Vec<String>,
    /// `(origin, missing seq)` — a gap above the compaction watermark,
    /// meaning an entry was deleted.
    pub gaps: Vec<(String, u64)>,
    /// `(origin, seq)` whose `prev` does not match its predecessor's
    /// hash.
    pub broken_links: Vec<(String, u64)>,
}

impl VerifyReport {
    pub fn is_clean(&self) -> bool {
        self.unreadable.is_empty() && self.gaps.is_empty() && self.broken_links.is_empty()
    }
}

#[derive(Debug)]
struct LocalState {
    clock: HlcClock,
    next_seq: u64,
    prev_hash: Option<String>,
}

/// The library's authoritative change log.
pub struct ChangeLog {
    changes_dir: PathBuf,
    snapshots_dir: PathBuf,
    origin: InstallId,
    state: Mutex<LocalState>,
}

impl ChangeLog {
    /// Opens (creating if needed) the log for a library.
    ///
    /// Reads the log to find this origin's own tip, so appends continue
    /// its chain rather than restarting it — and to set the clock past
    /// every stamp already on disk, including peers', so nothing this
    /// process writes sorts before something it has already seen.
    pub fn open(library_path: &Path) -> Result<Self> {
        let base = library_path.join(LIBRARY_HANDLE_DIR_NAME);
        let changes_dir = base.join("changes");
        let snapshots_dir = base.join("snapshots");
        fs::create_dir_all(&changes_dir).with_context(|| format!("creating {}", changes_dir.display()))?;
        fs::create_dir_all(&snapshots_dir).with_context(|| format!("creating {}", snapshots_dir.display()))?;

        let origin = InstallId::load_or_create(&base)?;
        let log = ChangeLog {
            changes_dir,
            snapshots_dir,
            origin,
            state: Mutex::new(LocalState { clock: HlcClock::resuming_from(Hlc::ZERO), next_seq: 1, prev_hash: None }),
        };

        let replay = log.replay()?;
        let mut state = log.state.lock().unwrap();
        for change in replay.changes() {
            state.clock.observe(change.hlc);
            if change.origin == log.origin.0 && change.seq >= state.next_seq {
                state.next_seq = change.seq + 1;
                state.prev_hash = Some(change.hash.clone());
            }
        }
        drop(state);
        Ok(log)
    }

    pub fn origin(&self) -> &InstallId {
        &self.origin
    }

    /// Appends a change and returns the entry written.
    ///
    /// Durable before it returns: the entry file is fsynced, then the
    /// directory, so a crash immediately afterwards cannot leave an
    /// entry that exists in the page cache and not on disk. This is the
    /// authority for the library's metadata, so "probably written" is
    /// not good enough.
    pub fn append(&self, op: ChangeOp) -> Result<Change> {
        let mut state = self.state.lock().unwrap();
        let change = Change::new(state.clock.now(), self.origin.0.clone(), state.next_seq, state.prev_hash.clone(), op);

        let nonce = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
        let path = self.changes_dir.join(change.file_name(&nonce));
        let bytes = serde_json::to_vec(&change)?;

        let mut file = fs::File::create(&path).with_context(|| format!("creating {}", path.display()))?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        // The entry's *name* is only durable once the directory is
        // synced; without this a crash can lose a fully-written file.
        if let Ok(dir) = fs::File::open(&self.changes_dir) {
            let _ = dir.sync_all();
        }

        state.next_seq = change.seq + 1;
        state.prev_hash = Some(change.hash.clone());
        Ok(change)
    }

    /// Everything needed to rebuild the database, in apply order.
    pub fn replay(&self) -> Result<Replay> {
        let (header, snapshot) = self.latest_snapshot()?;
        let watermarks = header.map(|h| h.watermarks).unwrap_or_default();

        let mut tail: Vec<Change> = Vec::new();
        for change in self.read_all_changes()?.0 {
            // Skip what the snapshot already accounts for. Comparing
            // per origin, because a watermark for one peer says nothing
            // about another's.
            let covered = watermarks.get(&change.origin).is_some_and(|w| change.seq <= w.seq);
            if !covered {
                tail.push(change);
            }
        }
        tail.sort_by(total_order);
        Ok(Replay { snapshot, tail })
    }

    /// Checks every origin's chain.
    ///
    /// A gap or a broken link means an entry was deleted or edited
    /// outside compaction. Reported per problem rather than as a single
    /// error, so one bad file does not mask the rest of the log.
    pub fn verify(&self) -> Result<VerifyReport> {
        let (changes, unreadable) = self.read_all_changes()?;
        let (header, _) = self.latest_snapshot()?;
        let watermarks = header.map(|h| h.watermarks).unwrap_or_default();

        let mut by_origin: HashMap<String, Vec<Change>> = HashMap::new();
        for change in changes {
            by_origin.entry(change.origin.clone()).or_default().push(change);
        }

        let mut report = VerifyReport { unreadable, ..Default::default() };
        for (origin, mut entries) in by_origin {
            entries.sort_by_key(|c| c.seq);

            // Where this origin's surviving history is expected to
            // start, and what its first entry should point back at.
            let watermark = watermarks.get(&origin);
            let mut expected_seq = watermark.map(|w| w.seq + 1).unwrap_or(1);
            let mut expected_prev = watermark.map(|w| w.hash.clone());

            for entry in entries {
                if entry.seq < expected_seq {
                    // Below the watermark: compaction's business, not a
                    // gap.
                    continue;
                }
                while entry.seq > expected_seq {
                    report.gaps.push((origin.clone(), expected_seq));
                    expected_seq += 1;
                    // The chain cannot be checked across a gap.
                    expected_prev = None;
                }
                if expected_prev.is_some() && entry.prev != expected_prev {
                    report.broken_links.push((origin.clone(), entry.seq));
                }
                expected_prev = Some(entry.hash.clone());
                expected_seq = entry.seq + 1;
            }
        }
        report.gaps.sort();
        report.broken_links.sort();
        Ok(report)
    }

    /// Collapses changes older than `retention` into a snapshot and
    /// deletes the entries it accounts for.
    ///
    /// `now_ms` is supplied rather than read from the clock so the
    /// horizon is testable without waiting sixty days.
    pub fn compact(&self, retention: Duration, now_ms: u64) -> Result<CompactionReport> {
        let horizon_ms = now_ms.saturating_sub(retention.as_millis() as u64);

        let (all, _) = self.read_all_changes()?;
        let (old_header, old_snapshot) = self.latest_snapshot()?;
        let old_watermarks = old_header.as_ref().map(|h| h.watermarks.clone()).unwrap_or_default();

        // Candidates: the existing snapshot's contents plus every
        // uncompacted entry old enough to be past the horizon.
        let mut candidates: Vec<Change> = old_snapshot;
        let mut compactable_files: Vec<Change> = Vec::new();
        let mut report = CompactionReport::default();

        for change in all {
            if old_watermarks.get(&change.origin).is_some_and(|w| change.seq <= w.seq) {
                continue; // already in the snapshot
            }
            if change.hlc.wall_ms >= horizon_ms {
                report.within_horizon += 1;
                continue;
            }
            compactable_files.push(change);
        }
        candidates.extend(compactable_files.iter().cloned());
        candidates.sort_by(total_order);

        // Walk backwards keeping the last change per supersede key --
        // the later one alone produces the same state.
        let mut seen: HashSet<(String, String, String)> = HashSet::new();
        let mut kept: Vec<Change> = Vec::new();
        for change in candidates.iter().rev() {
            match change.op.supersede_key() {
                Some((book, kind, name)) => {
                    let key = (book.to_string(), kind.to_string(), name.to_string());
                    if seen.insert(key) {
                        kept.push(change.clone());
                    } else {
                        report.dropped += 1;
                    }
                }
                None => kept.push(change.clone()),
            }
        }
        kept.reverse();
        report.retained = kept.len();

        // New watermarks: the highest compacted sequence per origin,
        // with that entry's hash so verification can bridge the gap.
        let mut watermarks = old_watermarks;
        for change in &compactable_files {
            let entry = watermarks.entry(change.origin.clone()).or_insert(Watermark { seq: 0, hash: String::new() });
            if change.seq >= entry.seq {
                *entry = Watermark { seq: change.seq, hash: change.hash.clone() };
            }
        }

        let header = SnapshotHeader { watermarks, taken_at: Hlc { wall_ms: now_ms, counter: 0 } };
        self.write_snapshot(&header, &kept)?;

        // Only now that the snapshot is durable. The reverse order
        // would lose every collapsed change to a crash in between.
        for change in &compactable_files {
            if let Some(path) = self.find_change_file(change)? {
                let _ = fs::remove_file(path);
            }
        }
        Ok(report)
    }

    /// Reads and orders every change file, collecting the names of any
    /// that could not be read.
    fn read_all_changes(&self) -> Result<(Vec<Change>, Vec<String>)> {
        let mut changes = Vec::new();
        let mut unreadable = Vec::new();

        let entries = match fs::read_dir(&self.changes_dir) {
            Ok(entries) => entries,
            // A log directory that cannot be read is not an empty log:
            // that distinction is the difference between "no changes"
            // and "the disk is gone", and conflating them would replay
            // an empty library over a real one.
            Err(e) => return Err(e).with_context(|| format!("reading {}", self.changes_dir.display())),
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()).map(str::to_string) else { continue };
            if !name.ends_with(".json") {
                continue;
            }
            // Cloud-sync services rename their conflict copies; two
            // files with the same content would otherwise replay twice.
            if is_sync_conflict_copy(&name) {
                continue;
            }
            match fs::read(&path) {
                Ok(bytes) => match Change::from_json(&bytes) {
                    Ok(change) => changes.push(change),
                    Err(ChangeParseError::HashMismatch) | Err(ChangeParseError::Malformed(_)) => unreadable.push(name),
                },
                Err(_) => unreadable.push(name),
            }
        }
        changes.sort_by(total_order);
        Ok((changes, unreadable))
    }

    fn find_change_file(&self, change: &Change) -> Result<Option<PathBuf>> {
        let prefix = format!("{}-{}-", change.hlc.file_prefix(), change.origin);
        for entry in fs::read_dir(&self.changes_dir)?.flatten() {
            let path = entry.path();
            if path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(&prefix)) {
                return Ok(Some(path));
            }
        }
        Ok(None)
    }

    /// The newest snapshot, as `(header, collapsed changes)`.
    ///
    /// Newest by filename, which is the stamp it was taken at.
    fn latest_snapshot(&self) -> Result<(Option<SnapshotHeader>, Vec<Change>)> {
        let mut newest: Option<PathBuf> = None;
        if let Ok(entries) = fs::read_dir(&self.snapshots_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                if newest.as_ref().is_none_or(|current| path.file_name() > current.file_name()) {
                    newest = Some(path);
                }
            }
        }
        let Some(path) = newest else { return Ok((None, Vec::new())) };

        let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let mut lines = text.lines();
        let Some(header_line) = lines.next() else { return Ok((None, Vec::new())) };
        let header: SnapshotHeader = serde_json::from_str(header_line).with_context(|| format!("reading the header of {}", path.display()))?;

        let mut changes = Vec::new();
        for line in lines {
            if line.trim().is_empty() {
                continue;
            }
            match Change::from_json(line.as_bytes()) {
                Ok(change) => changes.push(change),
                // A damaged snapshot line must not be skipped
                // silently: unlike a loose change file, there is no
                // other copy of it.
                Err(e) => anyhow::bail!("{} contains an unreadable change: {e}", path.display()),
            }
        }
        changes.sort_by(total_order);
        Ok((Some(header), changes))
    }

    /// Writes a snapshot as JSONL: header, then one change per line.
    ///
    /// Line-per-record rather than one big JSON array so it stays
    /// greppable and a human can see what the library thinks happened —
    /// which is the point of keeping the authority in a text format at
    /// all.
    fn write_snapshot(&self, header: &SnapshotHeader, changes: &[Change]) -> Result<PathBuf> {
        let name = format!("{}.jsonl", header.taken_at.file_prefix());
        let final_path = self.snapshots_dir.join(&name);
        let temp_path = self.snapshots_dir.join(format!("{name}.partial"));

        let mut out = Vec::new();
        out.extend_from_slice(&serde_json::to_vec(header)?);
        out.push(b'\n');
        for change in changes {
            out.extend_from_slice(&serde_json::to_vec(change)?);
            out.push(b'\n');
        }

        // Written to a temporary name and renamed, so a crash mid-write
        // cannot leave a half-snapshot that `latest_snapshot` would
        // pick up and refuse to read.
        let mut file = fs::File::create(&temp_path)?;
        file.write_all(&out)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp_path, &final_path)?;
        if let Ok(dir) = fs::File::open(&self.snapshots_dir) {
            let _ = dir.sync_all();
        }
        Ok(final_path)
    }
}

/// Whether a filename is a sync service's conflict copy rather than a
/// real entry.
///
/// Dropbox writes `name (conflicted copy 2024-01-01).json`, OneDrive
/// `name-MACHINE.json`, Nextcloud `name (conflicted copy).json`. Each is
/// a byte-identical or near-identical duplicate of a change that is
/// already present, so replaying it would apply the same change twice —
/// harmless for a field set, not harmless for anything counted.
pub fn is_sync_conflict_copy(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("conflicted copy") || lower.contains("conflict copy") || lower.contains(".sb-") || lower.contains("(case conflict")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(book: &str, field: &str, value: &str) -> ChangeOp {
        ChangeOp::FieldSet { book: book.into(), field: field.into(), value: Some(value.into()) }
    }

    fn log() -> (tempfile::TempDir, ChangeLog) {
        let dir = tempfile::tempdir().unwrap();
        let log = ChangeLog::open(dir.path()).unwrap();
        (dir, log)
    }

    #[test]
    fn appending_then_replaying_returns_what_was_written() {
        let (_dir, log) = log();
        log.append(ChangeOp::BookAdded { book: "u1".into() }).unwrap();
        log.append(field("u1", "title", "Dune")).unwrap();

        let replay = log.replay().unwrap();
        assert_eq!(replay.len(), 2);
        assert_eq!(replay.changes().next().unwrap().op, ChangeOp::BookAdded { book: "u1".into() });
    }

    #[test]
    fn an_install_id_is_stable_across_opens() {
        let dir = tempfile::tempdir().unwrap();
        let first = ChangeLog::open(dir.path()).unwrap().origin().clone();
        let second = ChangeLog::open(dir.path()).unwrap().origin().clone();
        assert_eq!(first, second, "a new id per run would make every restart look like a new peer");
    }

    /// Reopening must continue this origin's chain, not restart it --
    /// a restarted chain is indistinguishable from a tampered one.
    #[test]
    fn reopening_continues_the_chain() {
        let dir = tempfile::tempdir().unwrap();
        {
            let log = ChangeLog::open(dir.path()).unwrap();
            log.append(field("u1", "title", "One")).unwrap();
            log.append(field("u1", "title", "Two")).unwrap();
        }
        let log = ChangeLog::open(dir.path()).unwrap();
        let third = log.append(field("u1", "title", "Three")).unwrap();
        assert_eq!(third.seq, 3);

        let report = log.verify().unwrap();
        assert!(report.is_clean(), "{report:?}");
    }

    #[test]
    fn replay_is_in_clock_order_regardless_of_read_order() {
        let (_dir, log) = log();
        for i in 0..20 {
            log.append(field("u1", "title", &format!("v{i}"))).unwrap();
        }
        let replay = log.replay().unwrap();
        let stamps: Vec<Hlc> = replay.changes().map(|c| c.hlc).collect();
        let mut sorted = stamps.clone();
        sorted.sort();
        assert_eq!(stamps, sorted);
    }

    #[test]
    fn a_clean_log_verifies() {
        let (_dir, log) = log();
        log.append(field("u1", "title", "Dune")).unwrap();
        log.append(field("u1", "rating", "8")).unwrap();
        assert!(log.verify().unwrap().is_clean());
    }

    #[test]
    fn a_deleted_entry_shows_up_as_a_gap() {
        let (dir, log) = log();
        log.append(field("u1", "title", "One")).unwrap();
        let second = log.append(field("u1", "title", "Two")).unwrap();
        log.append(field("u1", "title", "Three")).unwrap();

        let path = log.find_change_file(&second).unwrap().unwrap();
        fs::remove_file(path).unwrap();

        let report = log.verify().unwrap();
        assert_eq!(report.gaps, vec![(log.origin().as_str().to_string(), 2)]);
        let _ = dir;
    }

    #[test]
    fn an_edited_entry_is_reported_as_unreadable() {
        let (_dir, log) = log();
        let change = log.append(field("u1", "title", "Dune")).unwrap();
        let path = log.find_change_file(&change).unwrap().unwrap();
        let text = fs::read_to_string(&path).unwrap().replace("Dune", "Duno");
        fs::write(&path, text).unwrap();

        let report = log.verify().unwrap();
        assert_eq!(report.unreadable.len(), 1, "{report:?}");
        // And it is excluded from replay rather than applied.
        assert!(log.replay().unwrap().is_empty());
    }

    /// The property that makes the log the authority rather than the
    /// database: everything needed to rebuild is in the log.
    #[test]
    fn a_replay_survives_losing_everything_but_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let expected: Vec<Change> = {
            let log = ChangeLog::open(dir.path()).unwrap();
            log.append(ChangeOp::BookAdded { book: "u1".into() }).unwrap();
            log.append(field("u1", "title", "Dune")).unwrap();
            log.append(ChangeOp::FormatSet { book: "u1".into(), format: "PDF".into(), name: "dune".into(), size: 10, hash: Some("abc".into()) }).unwrap();
            log.replay().unwrap().changes().cloned().collect()
        };

        // Stand-in for `rm metadata.db`: nothing but the log survives.
        fs::write(dir.path().join("metadata.db"), b"pretend database").unwrap();
        fs::remove_file(dir.path().join("metadata.db")).unwrap();

        let reopened = ChangeLog::open(dir.path()).unwrap();
        let actual: Vec<Change> = reopened.replay().unwrap().changes().cloned().collect();
        assert_eq!(actual, expected);
    }

    // ---- compaction ----

    const DAY_MS: u64 = 86_400_000;

    /// Appends with a stamp far enough in the past to be compactable,
    /// bypassing the live clock.
    fn append_at(log: &ChangeLog, wall_ms: u64, seq: u64, prev: Option<String>, op: ChangeOp) -> Change {
        let change = Change::new(Hlc { wall_ms, counter: 0 }, log.origin().as_str().to_string(), seq, prev, op);
        let path = log.changes_dir.join(change.file_name("00000000"));
        fs::write(path, serde_json::to_vec(&change).unwrap()).unwrap();
        change
    }

    #[test]
    fn compaction_collapses_superseded_field_writes() {
        let (_dir, log) = log();
        let now = 100 * DAY_MS;
        let a = append_at(&log, DAY_MS, 1, None, field("u1", "title", "First"));
        let b = append_at(&log, 2 * DAY_MS, 2, Some(a.hash.clone()), field("u1", "title", "Second"));
        append_at(&log, 3 * DAY_MS, 3, Some(b.hash.clone()), field("u1", "title", "Third"));

        let report = log.compact(DEFAULT_RETENTION, now).unwrap();
        assert_eq!(report.dropped, 2);
        assert_eq!(report.retained, 1);

        // The surviving state is the last write, and replay still
        // produces it.
        let replay = log.replay().unwrap();
        assert_eq!(replay.len(), 1);
        assert_eq!(replay.changes().next().unwrap().op, field("u1", "title", "Third"));
    }

    #[test]
    fn compaction_keeps_different_fields_and_different_books() {
        let (_dir, log) = log();
        let a = append_at(&log, DAY_MS, 1, None, field("u1", "title", "T"));
        let b = append_at(&log, 2 * DAY_MS, 2, Some(a.hash.clone()), field("u1", "rating", "8"));
        append_at(&log, 3 * DAY_MS, 3, Some(b.hash.clone()), field("u2", "title", "Other"));

        log.compact(DEFAULT_RETENTION, 100 * DAY_MS).unwrap();
        assert_eq!(log.replay().unwrap().len(), 3);
    }

    /// The failure this horizon exists to prevent: a laptop offline for
    /// a month holds the only copy of its own edits, and compaction
    /// must not delete them from under it.
    #[test]
    fn compaction_leaves_recent_changes_alone() {
        let (_dir, log) = log();
        let now = 100 * DAY_MS;
        append_at(&log, now - DAY_MS, 1, None, field("u1", "title", "Recent"));

        let report = log.compact(DEFAULT_RETENTION, now).unwrap();
        assert_eq!(report.within_horizon, 1);
        assert_eq!(report.retained, 0);
        // Still a loose file, still replayed.
        assert_eq!(log.replay().unwrap().len(), 1);
    }

    #[test]
    fn a_removal_is_never_collapsed_away() {
        let (_dir, log) = log();
        let a = append_at(&log, DAY_MS, 1, None, ChangeOp::BookAdded { book: "u1".into() });
        let b = append_at(&log, 2 * DAY_MS, 2, Some(a.hash.clone()), field("u1", "title", "Dune"));
        append_at(&log, 3 * DAY_MS, 3, Some(b.hash.clone()), ChangeOp::BookRemoved { book: "u1".into() });

        log.compact(DEFAULT_RETENTION, 100 * DAY_MS).unwrap();
        let replay = log.replay().unwrap();
        let ops: Vec<&ChangeOp> = replay.snapshot.iter().map(|c| &c.op).collect();
        // Dropping the removal would resurrect the book; dropping the
        // add would leave the field write pointing at nothing.
        assert!(ops.contains(&&ChangeOp::BookRemoved { book: "u1".into() }), "{ops:?}");
        assert!(ops.contains(&&ChangeOp::BookAdded { book: "u1".into() }), "{ops:?}");
    }

    #[test]
    fn a_compacted_log_still_verifies_clean() {
        let (_dir, log) = log();
        let a = append_at(&log, DAY_MS, 1, None, field("u1", "title", "One"));
        let b = append_at(&log, 2 * DAY_MS, 2, Some(a.hash.clone()), field("u1", "title", "Two"));
        append_at(&log, 3 * DAY_MS, 3, Some(b.hash.clone()), field("u1", "rating", "8"));

        log.compact(DEFAULT_RETENTION, 100 * DAY_MS).unwrap();

        // Compaction deletes entries 1 and 2, which without a
        // watermark would read as two gaps.
        let report = log.verify().unwrap();
        assert!(report.is_clean(), "compaction must not look like tampering: {report:?}");
    }

    #[test]
    fn appending_after_compaction_continues_the_chain() {
        let dir = tempfile::tempdir().unwrap();
        {
            let log = ChangeLog::open(dir.path()).unwrap();
            let a = append_at(&log, DAY_MS, 1, None, field("u1", "title", "One"));
            append_at(&log, 2 * DAY_MS, 2, Some(a.hash.clone()), field("u1", "title", "Two"));
            log.compact(DEFAULT_RETENTION, 100 * DAY_MS).unwrap();
        }
        let log = ChangeLog::open(dir.path()).unwrap();
        let next = log.append(field("u1", "rating", "9")).unwrap();
        assert_eq!(next.seq, 3, "the sequence must continue past what compaction absorbed");
        assert!(log.verify().unwrap().is_clean());
    }

    #[test]
    fn compacting_twice_is_stable() {
        let (_dir, log) = log();
        let a = append_at(&log, DAY_MS, 1, None, field("u1", "title", "One"));
        append_at(&log, 2 * DAY_MS, 2, Some(a.hash.clone()), field("u1", "title", "Two"));

        log.compact(DEFAULT_RETENTION, 100 * DAY_MS).unwrap();
        let after_first = log.replay().unwrap();
        log.compact(DEFAULT_RETENTION, 101 * DAY_MS).unwrap();
        let after_second = log.replay().unwrap();

        assert_eq!(after_first.changes().collect::<Vec<_>>(), after_second.changes().collect::<Vec<_>>());
    }

    // ---- merge ----

    /// The payoff: two machines' logs delivered into one directory by a
    /// file sync converge, with no coordination between them.
    #[test]
    fn two_peers_logs_merge_by_copying_files() {
        let alice_dir = tempfile::tempdir().unwrap();
        let bob_dir = tempfile::tempdir().unwrap();

        let alice = ChangeLog::open(alice_dir.path()).unwrap();
        let bob = ChangeLog::open(bob_dir.path()).unwrap();
        assert_ne!(alice.origin(), bob.origin(), "two installs must not share an origin");

        alice.append(field("u1", "title", "From Alice")).unwrap();
        bob.append(field("u1", "rating", "8")).unwrap();

        // What a sync service does: copy each side's files to the other.
        for entry in fs::read_dir(&bob.changes_dir).unwrap().flatten() {
            fs::copy(entry.path(), alice.changes_dir.join(entry.file_name())).unwrap();
        }

        let merged = ChangeLog::open(alice_dir.path()).unwrap();
        let replay = merged.replay().unwrap();
        assert_eq!(replay.len(), 2, "both peers' changes should survive the merge");
        assert!(merged.verify().unwrap().is_clean(), "each origin's chain is verified separately");
    }

    /// Two peers minting the same stamp in the same millisecond must
    /// still write different files. A single monotonic sequence number
    /// -- what the file-operation journal uses -- would have both write
    /// the same name and one silently overwrite the other.
    #[test]
    fn identical_stamps_from_two_peers_do_not_overwrite_each_other() {
        let (_dir, log) = log();
        let a = Change::new(Hlc { wall_ms: 1_000, counter: 0 }, "aaaaaaaaaaaaaaaa".into(), 1, None, field("u1", "title", "A"));
        let b = Change::new(Hlc { wall_ms: 1_000, counter: 0 }, "bbbbbbbbbbbbbbbb".into(), 1, None, field("u1", "title", "B"));
        for change in [&a, &b] {
            fs::write(log.changes_dir.join(change.file_name("00000000")), serde_json::to_vec(change).unwrap()).unwrap();
        }
        assert_eq!(log.replay().unwrap().len(), 2);
    }

    #[test]
    fn a_sync_conflict_copy_is_ignored_rather_than_replayed_twice() {
        let (_dir, log) = log();
        let change = log.append(field("u1", "title", "Dune")).unwrap();
        let path = log.find_change_file(&change).unwrap().unwrap();
        let copy = log.changes_dir.join("0000000000001-00000-aaaa (conflicted copy 2026-01-01).json");
        fs::copy(&path, &copy).unwrap();

        assert_eq!(log.replay().unwrap().len(), 1, "the conflict copy replayed as a second change");
    }

    #[test]
    fn conflict_copy_names_are_recognised_across_services() {
        assert!(is_sync_conflict_copy("x (conflicted copy 2026-01-01).json"));
        assert!(is_sync_conflict_copy("x (Conflicted Copy).json"));
        assert!(is_sync_conflict_copy("x.sb-1234abcd-XyZ.json"));
        assert!(!is_sync_conflict_copy("0000000000001-00000-aaaa-bbbb.json"));
    }

    /// A merge must pull the clock past the peer's stamps, or a local
    /// edit made afterwards would sort before a change already seen --
    /// and silently lose.
    #[test]
    fn opening_a_merged_log_advances_the_clock_past_peer_stamps() {
        let (_dir, log) = log();
        let far_future = Hlc { wall_ms: 4_000_000_000_000, counter: 0 };
        let peer = Change::new(far_future, "ffffffffffffffff".into(), 1, None, field("u1", "title", "From the future"));
        fs::write(log.changes_dir.join(peer.file_name("00000000")), serde_json::to_vec(&peer).unwrap()).unwrap();

        let reopened = ChangeLog::open(log_dir_of(&log)).unwrap();
        let mine = reopened.append(field("u1", "title", "Mine, and later")).unwrap();
        assert!(mine.hlc > far_future, "{:?} should sort after {far_future:?}", mine.hlc);
    }

    /// `changes_dir` is `<library>/.calibre-oxide/changes`.
    fn log_dir_of(log: &ChangeLog) -> &Path {
        log.changes_dir.parent().unwrap().parent().unwrap()
    }

    #[test]
    fn an_unreadable_log_directory_is_an_error_not_an_empty_log() {
        let dir = tempfile::tempdir().unwrap();
        let log = ChangeLog::open(dir.path()).unwrap();
        fs::remove_dir_all(&log.changes_dir).unwrap();
        // Conflating "cannot read" with "nothing there" would replay an
        // empty library over a real one.
        assert!(log.replay().is_err());
    }
}
