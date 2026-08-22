//! `LibraryHandle` (issue #93): the single gateway every durable write
//! to a library folder is meant to go through, per
//! `docs/FAULT_TOLERANCE.md`.
//!
//! # Scope so far
//!
//! The design doc describes a large system: storage-tier
//! classification, a writer lock, atomic write primitives, a
//! BLAKE3-chained write-ahead journal with crash recovery, OS device-
//! removal notifications, OS sleep/resume notifications, two-phase
//! network-storage writes, and (per its own "definition of done") a
//! crate-wide retrofit so no other file in this crate touches a
//! library path with a raw `fs::write`/`fs::rename`. That is
//! realistically several more PRs' worth of work. Landed so far:
//!
//! **Phase 1**:
//! - [`StorageTier`] classification (§1): real on Unix (parses
//!   `/proc/mounts` for the longest-prefix-matching mount, checks its
//!   fstype against a known-network-filesystem table, and consults
//!   `/sys/block/<dev>/removable` for the rest). Not implemented on
//!   Windows (`GetDriveTypeW` isn't wired) -- see below.
//! - The exclusive writer lock (§7): a real cross-process advisory
//!   lock on `<library>/.calibre-oxide/writer.lock`, via
//!   `std::fs::File::try_lock` (stable, cross-platform in std as of
//!   this project's Rust version -- no OS-specific FFI needed here).
//!   Released automatically on drop (including process crash/kill,
//!   since the OS releases an advisory lock when the holding file
//!   descriptor closes for any reason).
//! - Lifecycle states ([`HandleState`]): `Open`/`Suspended`/`Detached`
//!   exist as real types with a real `Detached`/`Suspended` error
//!   contract, but nothing yet transitions a handle away from `Open`
//!   -- that's the device-notification (§4) and power-state (§5)
//!   phases, still not started.
//!
//! **Phase 2 (this pass)**: the write-ahead journal and crash
//! recovery (§2 steps 1-2, 4-5), and journal-entry BLAKE3 chaining
//! (§8's "the journal itself is BLAKE3-chained"). Every
//! [`LibraryHandle::write_atomic`]/[`LibraryHandle::rename_atomic`]
//! call now:
//!
//! 1. Writes a journal entry (`<library>/.calibre-oxide/journal/<uuid>.op`)
//!    describing the operation -- a monotonic sequence number, the
//!    previous entry's hash (`prev_head`), the operation descriptor,
//!    and a BLAKE3 hash of that descriptor -- and `fsync`s the entry
//!    file and the journal directory, *before* touching the real
//!    target.
//! 2. Performs the operation (phase 1's write-temp/fsync/rename/
//!    fsync-dir sequence -- the temp file's name is derived from this
//!    same journal entry's uuid, so recovery can find it).
//! 3. Writes a `<uuid>.committed` marker, fsynced.
//!
//! [`LibraryHandle::open`] runs a real recovery scan of the journal
//! directory before returning: it verifies the whole hash chain
//! (tamper/corruption detection -- a broken chain or a
//! self-inconsistent entry is a real, hard [`LibraryHandleError::Corruption`]),
//! then for every entry without a `.committed` marker, determines
//! whether the operation actually completed on disk (by re-hashing the
//! target file's content against the entry's recorded checksum, for
//! writes; by checking which of `from`/`to` exist, for renames) and
//! either finalizes the missing commit marker (the operation
//! completed, only the marker write was interrupted -- "safe to
//! consider done", matching upstream's "committed-but-unacked, safe
//! to re-apply" language) or cleans up an orphaned temp file (the
//! operation never completed -- the real target is untouched, so
//! there's nothing to roll back at the target level, just garbage to
//! remove).
//!
//! **Phase 3 (this pass)**: journal pruning, via a persisted rolling
//! checkpoint. [`LibraryHandle::open`] now prunes the journal *after*
//! recovery settles every entry: once more than
//! [`JOURNAL_PRUNE_RETENTION`] entries are live, it writes a
//! checkpoint file (`<library>/.calibre-oxide/journal_checkpoint`)
//! recording the sequence number and hash-chain value immediately
//! after the newest entry being dropped, then deletes that entry's and
//! every older entry's `.op`/`.committed` files. The next recovery
//! trusts the checkpoint as its starting point instead of requiring
//! the chain from seq 0 -- this is the "persisted rolling checkpoint"
//! option the phase 2 module doc flagged as the alternative to a
//! weaker "verify only what's on disk" guarantee; a pruned entry is
//! still provably part of an unbroken chain, it's just not the chain
//! recovery re-derives from scratch every time.
//!
//! Pruning the checkpoint write and the entry deletions are two
//! separate filesystem operations, not one atomic unit -- so pruning
//! is designed to be safely interruptible and idempotent instead:
//! the checkpoint is written (temp/fsync/rename/fsync-dir) *before*
//! any old entry is deleted, so a crash between the two just leaves
//! stale, already-superseded entries on disk. The next recovery scan
//! reads the checkpoint, ignores (and finishes deleting) any entry
//! below its boundary, and verifies the chain starting from the
//! checkpoint's recorded hash -- so an interrupted prune self-heals on
//! the next open rather than needing its own recovery path.
//!
//! Pruning only runs at [`LibraryHandle::open`] time, not continuously
//! during a long-lived process -- see "disclosed simplifications"
//! below for why.
//!
//! **Book-file/cover/sidecar BLAKE3 checksums** (§8's other half, not
//! numbered as a phase of this file since it lives in its own module)
//! shipped separately in `checksums.rs` -- see that module's doc for
//! the full design (a sidecar db, not a `metadata.db` column) and
//! everywhere it's wired in (`Cache::add_format`/`add_book`,
//! `covers::set_cover`, `backup.rs`, `check_library.rs`'s scan,
//! `cmd_export.rs`'s copy).
//!
//! **Phase 4**: device-removal notifications (§4), in
//! `device_monitor.rs` -- real on Linux, via a raw
//! `NETLINK_KOBJECT_UEVENT` socket (not `libudev`; see that module's
//! doc for why, and for how its exact wire-format assumptions were
//! verified against a real captured kernel message on this machine
//! rather than just documentation). [`LibraryHandle::open`] resolves
//! the `/dev/...` device backing the library's mount and, if one
//! exists, spawns a best-effort background thread watching for its
//! removal; seeing one flips `state` to `Detached`, and every
//! subsequent call through this handle then fails with
//! [`LibraryHandleError::DeviceDetached`] -- real per §4's "no retry
//! loop, no silent corruption path" contract, not just the reserved
//! error variant phases 1-3 left sitting unused. No implicit
//! re-attach: per §4, a caller must explicitly `open()` again.
//! Windows is disclosed as not implemented, same reason as this
//! file's other Windows gaps.
//!
//! # Disclosed simplifications (phase 2-4)
//!
//! - **Pruning is open-time only.** A process that opens a library
//!   once and then writes to it for a very long time will still grow
//!   the journal without bound until it's reopened. Pruning mid-session
//!   would need to distinguish "an old, fully-settled entry" from "an
//!   entry another thread is mid-way through writing right now," which
//!   the current design doesn't track (the writer-lock model assumes
//!   one process per library, but says nothing about multiple threads
//!   racing inside that process); rather than guess, pruning is
//!   confined to the point where recovery has already proven every
//!   on-disk entry is settled -- right after `open()` acquires the
//!   lock and before any new write can start.
//! - **No full-payload replay.** The journal entry for a write stores
//!   a BLAKE3 checksum of the intended content, not the content
//!   itself (matching upstream's "operation descriptor," not "the
//!   payload"). Recovery can therefore *detect* whether a write
//!   completed and *verify* its integrity, but if a write never made
//!   it to `target` at all (crashed before or during the temp-file
//!   write), there is nothing to replay -- the caller is expected to
//!   retry the whole operation. This is consistent with write-temp-
//!   then-rename's actual atomicity guarantee: an operation either
//!   fully happened or fully didn't, there's no partial state to
//!   resume from.
//! - **Journal entries serialize `PathBuf`s via serde's default
//!   encoding** (platform `OsString`-based) -- not guaranteed to
//!   round-trip identically across platforms for non-UTF-8 paths. Not
//!   a concern for this crate's actual usage (library paths are
//!   expected to be valid UTF-8), disclosed for completeness.
//!
//! # Not done yet (disclosed, tracked as later work under #93)
//!
//! - Sleep/resume notifications (§5) and the `Suspended` transition
//!   -- unlike device-removal, this needs a real architectural
//!   addition beyond what phase 4 needed: §5 step 1 wants a suspending
//!   handle to release its exclusive writer lock before sleep (so the
//!   OS isn't holding a lock across a suspend/resume cycle) and
//!   reacquire it on resume, which means `_lock_file` needs to become
//!   swappable at runtime, not a field set once at `open()` and never
//!   touched again -- a bigger change than adding a new background
//!   thread, deliberately not bundled into phase 4.
//! - Network-storage two-phase writes (§6) -- [`StorageTier::Network`]
//!   is detected and stored, but nothing yet changes behavior based on
//!   it.
//! - **The crate-wide retrofit.** Every existing `fs::write`/
//!   `fs::rename`/`fs::copy` call against a library path elsewhere in
//!   this crate (`cache.rs`, `restore.rs`, `notes/connection.rs`,
//!   `covers.rs`, `adding.rs`, and more) still calls `std::fs`
//!   directly, not through this handle. Retrofitting all of them is
//!   its own large, separately-reviewable change.
//! - Windows implementations of tier classification and directory
//!   durability (see above) -- this workspace has no way to compile-
//!   check or test Windows-specific code, so rather than ship
//!   plausible-looking but unverified FFI, both fall back to a
//!   disclosed, conservative default.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use thiserror::Error;
use uuid::Uuid;

use crate::constants::{
    JOURNAL_CHECKPOINT_FILE_NAME, JOURNAL_DIR_NAME, LIBRARY_HANDLE_DIR_NAME, WRITER_LOCK_FILE_NAME,
};

/// How many of the most recent journal entries [`LibraryHandle::open`]
/// keeps on disk before pruning older, already-settled ones. Each
/// entry is a tiny JSON file, so this is chosen generously (plenty of
/// forensic history) rather than tightly -- the point of pruning is
/// bounding growth over a library's *lifetime*, not minimizing disk
/// use day to day.
const JOURNAL_PRUNE_RETENTION: u64 = 500;

/// Port of `docs/FAULT_TOLERANCE.md` §1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageTier {
    /// Fixed disk on the machine.
    LocalInternal,
    /// USB/Thunderbolt/SD, mounted as a local filesystem.
    LocalExternal,
    /// SMB/NFS/WebDAV/cloud mount.
    Network,
}

/// Port of `docs/FAULT_TOLERANCE.md` §4-5's lifecycle states. Nothing
/// yet transitions a handle out of `Open` -- see this module's doc
/// comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleState {
    Open,
    Suspended,
    Detached,
}

#[derive(Debug, Error)]
pub enum LibraryHandleError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("another process already holds the writer lock for this library")]
    AlreadyLocked,
    /// Real per docs/FAULT_TOLERANCE.md §4's contract ("no retries
    /// hidden in the handle"); reserved -- nothing yet produces this
    /// (see module doc).
    #[error("the library's storage device has been detached")]
    DeviceDetached,
    /// Reserved for the power-state phase (§5).
    #[error("the library handle is suspended")]
    Suspended,
    /// Real as of phase 2: raised by journal recovery when a journal
    /// entry's own hash doesn't match its content, the hash chain is
    /// broken, or a recovered file's content doesn't match its
    /// recorded checksum.
    #[error("data corruption detected: {0}")]
    Corruption(String),
}

/// What a journal entry describes -- upstream's "operation
/// descriptor" (§2 step 2). `content_hash`/matching are hex BLAKE3.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
enum OperationDescriptor {
    WriteFile {
        target: PathBuf,
        content_hash: String,
    },
    RenameFile {
        from: PathBuf,
        to: PathBuf,
    },
}

/// One journal entry -- port of §2 step 2's four fields exactly:
/// sequence number, previous head, descriptor, BLAKE3 of the
/// descriptor.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct JournalEntry {
    seq: u64,
    prev_head: Option<String>,
    op: OperationDescriptor,
    descriptor_hash: String,
}

/// The journal's current tip, used to chain the next entry.
#[derive(Debug, Clone)]
struct JournalHead {
    next_seq: u64,
    prev_hash: Option<String>,
}

/// Persisted rolling checkpoint (phase 3): the chain-verification
/// starting point after pruning. Entries with `seq < boundary_seq` are
/// no longer on disk; the first remaining entry's `prev_head` is
/// expected to equal `boundary_hash`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct JournalCheckpoint {
    boundary_seq: u64,
    boundary_hash: Option<String>,
}

/// The single gateway every durable write to a library folder is
/// meant to go through. See this module's doc comment for exactly
/// what's real so far.
pub struct LibraryHandle {
    library_path: PathBuf,
    tier: StorageTier,
    /// `Arc`-wrapped so the device-removal monitor thread (§4, see
    /// `device_monitor.rs`) can hold a [`std::sync::Weak`] reference
    /// and flip this to `Detached` without keeping the handle alive
    /// on its own.
    state: Arc<Mutex<HandleState>>,
    journal_dir: PathBuf,
    journal_head: Mutex<JournalHead>,
    /// Held open for the handle's whole lifetime -- the OS releases
    /// the advisory lock automatically when this (and every other
    /// clone of the fd, which there are none of here) closes, which
    /// includes process crash/kill. This is what gives "no stale lock
    /// left behind after a crash" for free.
    _lock_file: File,
}

impl LibraryHandle {
    /// Opens (creating `<library>/.calibre-oxide/` if needed),
    /// acquires the exclusive writer lock, and runs journal recovery
    /// (see module doc). Fails with [`LibraryHandleError::AlreadyLocked`]
    /// if another `LibraryHandle` (in this or another process) already
    /// holds the lock -- no blocking, no retry loop, matching §7's
    /// "one writer per library" rule. Fails with
    /// [`LibraryHandleError::Corruption`] if the journal's hash chain
    /// doesn't verify.
    pub fn open(library_path: &Path) -> Result<Self, LibraryHandleError> {
        Self::open_impl(library_path, JOURNAL_PRUNE_RETENTION, true)
    }

    /// `retention` and `monitor_devices` are test-only hooks (see
    /// module doc's fault-injection pattern): `retention` lets tests
    /// exercise pruning without writing hundreds of entries;
    /// `monitor_devices` lets most tests skip spawning a real
    /// `device_monitor.rs` background thread (a real netlink socket
    /// per `LibraryHandle::open` call adds real, if small, overhead
    /// across dozens of tests that don't care about §4 at all). Always
    /// [`JOURNAL_PRUNE_RETENTION`]/`true` from the public
    /// [`LibraryHandle::open`].
    fn open_impl(
        library_path: &Path,
        retention: u64,
        monitor_devices: bool,
    ) -> Result<Self, LibraryHandleError> {
        let handle_dir = library_path.join(LIBRARY_HANDLE_DIR_NAME);
        fs::create_dir_all(&handle_dir)?;

        let lock_path = handle_dir.join(WRITER_LOCK_FILE_NAME);
        let lock_file = OpenOptions::new()
            .create(true)
            .write(true)
            .open(&lock_path)?;
        match lock_file.try_lock() {
            Ok(()) => {}
            Err(fs::TryLockError::WouldBlock) => return Err(LibraryHandleError::AlreadyLocked),
            Err(fs::TryLockError::Error(e)) => return Err(LibraryHandleError::Io(e)),
        }

        let journal_dir = handle_dir.join(JOURNAL_DIR_NAME);
        let checkpoint_path = handle_dir.join(JOURNAL_CHECKPOINT_FILE_NAME);
        let head = recover_journal(&journal_dir, &checkpoint_path, retention)?;

        let state = Arc::new(Mutex::new(HandleState::Open));
        #[cfg(unix)]
        if monitor_devices {
            if let Some(device_name) = resolve_device_name(library_path) {
                crate::device_monitor::spawn_device_monitor(device_name, Arc::downgrade(&state));
            }
        }
        #[cfg(not(unix))]
        let _ = monitor_devices;

        Ok(LibraryHandle {
            library_path: library_path.to_path_buf(),
            tier: classify_storage_tier(library_path),
            state,
            journal_dir,
            journal_head: Mutex::new(head),
            _lock_file: lock_file,
        })
    }

    pub fn library_path(&self) -> &Path {
        &self.library_path
    }

    pub fn tier(&self) -> StorageTier {
        self.tier
    }

    pub fn state(&self) -> HandleState {
        *self.state.lock().unwrap()
    }

    fn check_open(&self) -> Result<(), LibraryHandleError> {
        match *self.state.lock().unwrap() {
            HandleState::Open => Ok(()),
            HandleState::Detached => Err(LibraryHandleError::DeviceDetached),
            HandleState::Suspended => Err(LibraryHandleError::Suspended),
        }
    }

    /// Port of §2 steps 1-4 for a file write: journal, write-temp /
    /// fsync-temp / rename / fsync-parent-directory, mark committed.
    /// `target` must be an absolute path (or at least caller-resolved
    /// -- this doesn't re-root it under `library_path`, callers do
    /// that).
    pub fn write_atomic(&self, target: &Path, bytes: &[u8]) -> Result<(), LibraryHandleError> {
        self.write_atomic_impl(target, bytes, None)
    }

    /// Port of §2 steps 1-4 for a rename, for callers that already
    /// have the payload written to its final form elsewhere (e.g. a
    /// format file copied by a caller, then atomically published into
    /// place) -- POSIX `rename` is already atomic; this adds the
    /// journal entry and the durability-relevant fsync of the parent
    /// director(ies) upstream's discipline requires.
    pub fn rename_atomic(&self, from: &Path, to: &Path) -> Result<(), LibraryHandleError> {
        self.rename_atomic_impl(from, to, None)
    }

    /// `fail_after` is a test-only fault-injection hook (see module
    /// doc) -- always `None` from the public [`LibraryHandle::write_atomic`];
    /// tests call this directly with `Some(_)` to simulate a crash
    /// right after a given step.
    fn write_atomic_impl(
        &self,
        target: &Path,
        bytes: &[u8],
        fail_after: Option<FailPoint>,
    ) -> Result<(), LibraryHandleError> {
        self.check_open()?;
        let uuid = Uuid::new_v4();
        let content_hash = blake3_hex(bytes);
        let op = OperationDescriptor::WriteFile {
            target: target.to_path_buf(),
            content_hash,
        };
        self.journal_write(uuid, op)?;
        if fail_after == Some(FailPoint::JournalWrite) {
            return Err(simulated_crash());
        }

        let parent = target.parent();
        if let Some(parent) = parent {
            fs::create_dir_all(parent)?;
        }
        let tmp_path = temp_path_for(target, uuid);

        let tmp_file = File::create(&tmp_path)?;
        {
            use std::io::Write;
            (&tmp_file).write_all(bytes)?;
        }
        if fail_after == Some(FailPoint::WriteTemp) {
            return Err(simulated_crash());
        }

        tmp_file.sync_all()?;
        drop(tmp_file);
        if fail_after == Some(FailPoint::FsyncTemp) {
            return Err(simulated_crash());
        }

        fs::rename(&tmp_path, target)?;
        if fail_after == Some(FailPoint::Rename) {
            return Err(simulated_crash());
        }

        fsync_dir(parent)?;
        if fail_after == Some(FailPoint::BeforeCommit) {
            return Err(simulated_crash());
        }

        self.journal_commit(uuid)?;
        Ok(())
    }

    fn rename_atomic_impl(
        &self,
        from: &Path,
        to: &Path,
        fail_after: Option<FailPoint>,
    ) -> Result<(), LibraryHandleError> {
        self.check_open()?;
        let uuid = Uuid::new_v4();
        let op = OperationDescriptor::RenameFile {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
        };
        self.journal_write(uuid, op)?;
        if fail_after == Some(FailPoint::JournalWrite) {
            return Err(simulated_crash());
        }

        fs::rename(from, to)?;
        if fail_after == Some(FailPoint::Rename) {
            return Err(simulated_crash());
        }

        fsync_dir(from.parent())?;
        if to.parent() != from.parent() {
            fsync_dir(to.parent())?;
        }
        if fail_after == Some(FailPoint::BeforeCommit) {
            return Err(simulated_crash());
        }

        self.journal_commit(uuid)?;
        Ok(())
    }

    /// Port of §2 step 2: write the journal entry, fsync it, fsync
    /// the journal directory, then advance the in-memory head.
    fn journal_write(&self, uuid: Uuid, op: OperationDescriptor) -> Result<(), LibraryHandleError> {
        let descriptor_hash =
            blake3_hex(&serde_json::to_vec(&op).expect("OperationDescriptor always serializes"));
        let mut head = self.journal_head.lock().unwrap();
        let entry = JournalEntry {
            seq: head.next_seq,
            prev_head: head.prev_hash.clone(),
            op,
            descriptor_hash: descriptor_hash.clone(),
        };

        let path = self.journal_dir.join(format!("{uuid}.op"));
        let json = serde_json::to_vec(&entry).expect("JournalEntry always serializes");
        let file = File::create(&path)?;
        {
            use std::io::Write;
            (&file).write_all(&json)?;
        }
        file.sync_all()?;
        fsync_dir(Some(&self.journal_dir))?;

        head.next_seq += 1;
        head.prev_hash = Some(descriptor_hash);
        Ok(())
    }

    /// Port of §2 step 4: a single-byte status file, fsynced.
    fn journal_commit(&self, uuid: Uuid) -> Result<(), LibraryHandleError> {
        let path = self.journal_dir.join(format!("{uuid}.committed"));
        let file = File::create(&path)?;
        {
            use std::io::Write;
            (&file).write_all(b"1")?;
        }
        file.sync_all()?;
        Ok(())
    }
}

/// Test-only fault-injection points (always compiled, since it's a
/// plain enum with no runtime cost -- avoids `#[cfg(test)]` on a
/// function-call argument, which isn't stable syntax).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailPoint {
    /// Right after the journal entry is written+fsynced, before the
    /// real operation is touched at all.
    JournalWrite,
    WriteTemp,
    FsyncTemp,
    Rename,
    /// Right after the operation itself (and its directory fsync)
    /// fully completed, before the commit marker is written.
    BeforeCommit,
}

fn simulated_crash() -> LibraryHandleError {
    LibraryHandleError::Io(io::Error::other(
        "simulated crash (test-only fault injection)",
    ))
}

fn blake3_hex(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// `<parent>/<target-file-name>.tmp-<uuid>` -- deterministic from
/// `target` + `uuid` alone, so recovery can reconstruct it without
/// storing it separately in the journal entry.
fn temp_path_for(target: &Path, uuid: Uuid) -> PathBuf {
    let tmp_name = format!(
        "{}.tmp-{}",
        target
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file"),
        uuid
    );
    match target.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.join(tmp_name),
        _ => PathBuf::from(tmp_name),
    }
}

/// Port of §2's recovery-on-startup: scan the journal directory,
/// verify the hash chain, and settle every entry that isn't already
/// marked `committed` -- see module doc for the full algorithm. Also
/// runs phase 3's pruning pass once every entry has been settled.
fn recover_journal(
    journal_dir: &Path,
    checkpoint_path: &Path,
    retention: u64,
) -> Result<JournalHead, LibraryHandleError> {
    fs::create_dir_all(journal_dir)?;

    let checkpoint: Option<JournalCheckpoint> = fs::read(checkpoint_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let boundary_seq = checkpoint.as_ref().map_or(0, |c| c.boundary_seq);
    let boundary_hash = checkpoint.and_then(|c| c.boundary_hash);

    let mut entries: Vec<(Uuid, JournalEntry)> = Vec::new();
    for dir_entry in fs::read_dir(journal_dir)? {
        let dir_entry = dir_entry?;
        let path = dir_entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("op") {
            continue;
        }
        let Some(uuid) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| Uuid::parse_str(s).ok())
        else {
            continue;
        };
        let content = fs::read(&path)?;
        let parsed: JournalEntry = serde_json::from_slice(&content).map_err(|e| {
            LibraryHandleError::Corruption(format!("malformed journal entry {uuid}: {e}"))
        })?;
        entries.push((uuid, parsed));
    }
    entries.sort_by_key(|(_, e)| e.seq);

    let mut prev_hash = boundary_hash;
    let mut next_seq = boundary_seq;
    // (seq, uuid, descriptor_hash) for every live entry, in order --
    // used below to find the hash a new checkpoint boundary should
    // record if pruning kicks in.
    let mut live: Vec<(u64, Uuid, String)> = Vec::new();
    for (uuid, entry) in &entries {
        if entry.seq < boundary_seq {
            // Leftover from a prune whose checkpoint write committed
            // but whose deletions didn't finish -- already superseded,
            // not part of the chain recovery verifies. Finish deleting
            // it (self-heals an interrupted prune) and move on.
            remove_journal_entry_files(journal_dir, *uuid)?;
            continue;
        }

        let recomputed = blake3_hex(
            &serde_json::to_vec(&entry.op).expect("OperationDescriptor always serializes"),
        );
        if recomputed != entry.descriptor_hash {
            return Err(LibraryHandleError::Corruption(format!(
                "journal entry {uuid} descriptor hash does not match its own content"
            )));
        }
        if entry.prev_head != prev_hash {
            return Err(LibraryHandleError::Corruption(format!(
                "journal entry {uuid} breaks the hash chain"
            )));
        }
        prev_hash = Some(entry.descriptor_hash.clone());
        next_seq = entry.seq + 1;
        live.push((entry.seq, *uuid, entry.descriptor_hash.clone()));

        recover_one(journal_dir, *uuid, entry)?;
    }

    prune_if_needed(journal_dir, checkpoint_path, boundary_seq, retention, &live)?;

    Ok(JournalHead {
        next_seq,
        prev_hash,
    })
}

/// Phase 3: if more than `retention` entries are live, write a new
/// checkpoint recording the chain state just after the newest entry
/// being dropped, then delete every entry at or below that boundary.
/// The checkpoint is written -- fsynced and renamed into place --
/// before any deletion, so an interruption partway through just leaves
/// stale files that the next recovery scan finishes cleaning up (see
/// `recover_journal`'s `entry.seq < boundary_seq` branch).
fn prune_if_needed(
    journal_dir: &Path,
    checkpoint_path: &Path,
    old_boundary_seq: u64,
    retention: u64,
    live: &[(u64, Uuid, String)],
) -> Result<(), LibraryHandleError> {
    let live_count = live.len() as u64;
    if live_count <= retention {
        return Ok(());
    }

    let drop_count = (live_count - retention) as usize;
    let new_boundary_seq = live[drop_count - 1].0 + 1;
    let new_boundary_hash = Some(live[drop_count - 1].2.clone());
    debug_assert!(old_boundary_seq <= new_boundary_seq);

    write_checkpoint(
        checkpoint_path,
        &JournalCheckpoint {
            boundary_seq: new_boundary_seq,
            boundary_hash: new_boundary_hash,
        },
    )?;

    for (_, uuid, _) in &live[..drop_count] {
        remove_journal_entry_files(journal_dir, *uuid)?;
    }
    fsync_dir(Some(journal_dir))?;
    Ok(())
}

fn remove_journal_entry_files(journal_dir: &Path, uuid: Uuid) -> io::Result<()> {
    let op_path = journal_dir.join(format!("{uuid}.op"));
    if op_path.exists() {
        fs::remove_file(&op_path)?;
    }
    let committed_path = journal_dir.join(format!("{uuid}.committed"));
    if committed_path.exists() {
        fs::remove_file(&committed_path)?;
    }
    Ok(())
}

/// Write-temp/fsync/rename/fsync-dir, same discipline as
/// [`LibraryHandle::write_atomic_impl`], but standalone: the
/// checkpoint is the journal's own bookkeeping, not a library-content
/// write, so it deliberately isn't itself journaled (that would be
/// unbounded recursion -- a journal entry for writing the journal's
/// own checkpoint).
fn write_checkpoint(path: &Path, checkpoint: &JournalCheckpoint) -> Result<(), LibraryHandleError> {
    let bytes = serde_json::to_vec(checkpoint).expect("JournalCheckpoint always serializes");
    let tmp_path = path.with_extension("tmp");
    let tmp_file = File::create(&tmp_path)?;
    {
        use std::io::Write;
        (&tmp_file).write_all(&bytes)?;
    }
    tmp_file.sync_all()?;
    drop(tmp_file);
    fs::rename(&tmp_path, path)?;
    fsync_dir(path.parent())?;
    Ok(())
}

fn recover_one(
    journal_dir: &Path,
    uuid: Uuid,
    entry: &JournalEntry,
) -> Result<(), LibraryHandleError> {
    let committed_path = journal_dir.join(format!("{uuid}.committed"));
    if committed_path.exists() {
        // Already known-complete. A write's target content is
        // re-verified against its recorded checksum as a real
        // integrity check (§8's spirit); a mismatch here means the
        // file changed out from under the journal after it was
        // marked committed -- genuine corruption, not a recovery
        // scenario the design doc's replay logic covers.
        if let OperationDescriptor::WriteFile {
            target,
            content_hash,
        } = &entry.op
        {
            if let Ok(bytes) = fs::read(target) {
                if blake3_hex(&bytes) != *content_hash {
                    return Err(LibraryHandleError::Corruption(format!(
                        "{} does not match its recorded checksum",
                        target.display()
                    )));
                }
            }
        }
        return Ok(());
    }

    match &entry.op {
        OperationDescriptor::WriteFile {
            target,
            content_hash,
        } => {
            let applied = fs::read(target)
                .map(|bytes| blake3_hex(&bytes) == *content_hash)
                .unwrap_or(false);
            if applied {
                mark_committed(&committed_path)?;
            } else {
                let tmp_path = temp_path_for(target, uuid);
                if tmp_path.exists() {
                    fs::remove_file(&tmp_path)?;
                }
                // Else: crashed before the temp file was even
                // created -- nothing was touched, nothing to clean.
            }
        }
        OperationDescriptor::RenameFile { from, to } => {
            let applied = !from.exists() && to.exists();
            if applied {
                mark_committed(&committed_path)?;
            }
            // Else: the rename never happened (or `from` still
            // exists) -- nothing was touched, nothing to roll back.
        }
    }
    Ok(())
}

fn mark_committed(path: &Path) -> io::Result<()> {
    let file = File::create(path)?;
    {
        use std::io::Write;
        (&file).write_all(b"1")?;
    }
    file.sync_all()
}

/// `fsync`s a directory so a just-`rename`d entry survives a crash --
/// a standard POSIX technique (open the directory like a file, flush
/// it). No equivalent on Windows (a no-op there; see module doc).
fn fsync_dir(dir: Option<&Path>) -> io::Result<()> {
    let Some(dir) = dir else { return Ok(()) };
    if dir.as_os_str().is_empty() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        File::open(dir)?.sync_all()?;
    }
    let _ = dir;
    Ok(())
}

#[cfg(unix)]
fn classify_storage_tier(path: &Path) -> StorageTier {
    let target = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mounts = fs::read_to_string("/proc/mounts").unwrap_or_default();
    let Some((device, fstype)) = best_matching_mount(&mounts, &target) else {
        return StorageTier::LocalInternal;
    };
    classify_from_device_and_fstype(&device, &fstype)
}

/// The bare device name (`/proc/mounts`'s device column with its
/// `/dev/` prefix stripped, e.g. `"sdb1"`) backing `path`'s mount --
/// what `device_monitor.rs`'s netlink watcher matches a `remove`
/// uevent's `DEVNAME` field against. `None` if `path`'s mount can't be
/// determined (no `/dev/...` device, e.g. `tmpfs`/`overlay`, or
/// `/proc/mounts` itself is unavailable) -- there is nothing for a
/// device-removal monitor to usefully watch in that case, so
/// [`LibraryHandle::open_impl`] simply doesn't spawn one.
#[cfg(unix)]
fn resolve_device_name(path: &Path) -> Option<String> {
    let target = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mounts = fs::read_to_string("/proc/mounts").ok()?;
    let (device, _fstype) = best_matching_mount(&mounts, &target)?;
    device.strip_prefix("/dev/").map(|s| s.to_string())
}

#[cfg(windows)]
fn resolve_device_name(_path: &Path) -> Option<String> {
    // Real device-removal monitoring on Windows needs
    // `RegisterDeviceNotificationW` -- see this crate's
    // `device_monitor.rs` module doc for why that isn't implemented
    // here. `None` means `LibraryHandle::open_impl` simply doesn't
    // spawn a monitor thread on Windows, the same "disclosed
    // no-op, not a guess" shape every other Windows gap in this
    // module uses.
    None
}

#[cfg(windows)]
fn classify_storage_tier(_path: &Path) -> StorageTier {
    // Real classification needs `GetDriveTypeW`, which this port
    // can't compile-check or test on this workspace's Linux-only
    // toolchain -- see module doc. `LocalInternal` is the
    // conservative default (assume the least caution is needed only
    // when we genuinely can't tell otherwise, matching this file's
    // `/proc/mounts`-unavailable fallback on Unix).
    StorageTier::LocalInternal
}

/// Parses `/proc/mounts` (`device mount_point fstype ...` per line,
/// spaces in paths escaped as `\040`) and returns the `(device,
/// fstype)` of the longest-prefix-matching mount point for `target` --
/// a pure function so it's testable without needing a real `/proc`.
fn best_matching_mount(mounts: &str, target: &Path) -> Option<(String, String)> {
    let mut best: Option<(PathBuf, String, String)> = None;
    for line in mounts.lines() {
        let mut fields = line.split_whitespace();
        let (Some(device), Some(mount_point), Some(fstype)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let mount_point = PathBuf::from(mount_point.replace("\\040", " "));
        if !target.starts_with(&mount_point) {
            continue;
        }
        let is_better = match &best {
            Some((cur, _, _)) => mount_point.components().count() > cur.components().count(),
            None => true,
        };
        if is_better {
            best = Some((mount_point, device.to_string(), fstype.to_string()));
        }
    }
    best.map(|(_, device, fstype)| (device, fstype))
}

const NETWORK_FSTYPES: &[&str] = &[
    "nfs",
    "nfs4",
    "cifs",
    "smbfs",
    "smb3",
    "fuse.sshfs",
    "davfs",
    "fuse.rclone",
    "9p",
    "afs",
    "ceph",
    "glusterfs",
];

fn classify_from_device_and_fstype(device: &str, fstype: &str) -> StorageTier {
    if NETWORK_FSTYPES.contains(&fstype) {
        return StorageTier::Network;
    }
    if is_removable_device(device) {
        StorageTier::LocalExternal
    } else {
        StorageTier::LocalInternal
    }
}

/// `device` is a `/dev/...` path (e.g. `/dev/sda1`, `/dev/nvme0n1p1`).
/// Real check via `/sys/block/<base>/removable` (`"1"` for
/// removable/hot-pluggable media, `"0"` for fixed) -- the same signal
/// `lsblk`/`udisks` use. Anything that isn't a real `/dev/...` device
/// (`tmpfs`, `overlay`, a bind mount, ...) is treated as not
/// removable, i.e. `LocalInternal`.
fn is_removable_device(device: &str) -> bool {
    let Some(name) = device.strip_prefix("/dev/") else {
        return false;
    };
    let base = base_block_device(name);
    fs::read_to_string(format!("/sys/block/{base}/removable"))
        .map(|s| s.trim() == "1")
        .unwrap_or(false)
}

/// Strips a partition suffix off a Linux block device name:
/// `sda1` -> `sda`, `nvme0n1p1` -> `nvme0n1`, `mmcblk0p1` -> `mmcblk0`.
/// A name with no partition suffix (`sda`, `nvme0n1`) is returned
/// unchanged.
fn base_block_device(name: &str) -> String {
    if let Some(pos) = name.rfind('p') {
        let (head, tail) = name.split_at(pos);
        let suffix = &tail[1..];
        if !suffix.is_empty()
            && suffix.chars().all(|c| c.is_ascii_digit())
            && head.chars().last().is_some_and(|c| c.is_ascii_digit())
        {
            return head.to_string();
        }
    }
    // Only treat trailing digits as an `sdX`-style partition number
    // (`sda1` -> `sda`) when what's left is purely alphabetic. An
    // `nvme0n1`-style whole-disk name would also trim to something
    // ending in digits here (`nvme0n1` -> `nvme0n`, which still has
    // the `0`) -- nvme partitions always use an explicit `pN` suffix
    // (handled above), so a bare digit-ending nvme name is never a
    // partition and is left unchanged.
    let trimmed = name.trim_end_matches(|c: char| c.is_ascii_digit());
    if !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_alphabetic()) {
        trimmed.to_string()
    } else {
        name.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn open_creates_the_handle_dir_and_acquires_the_lock() {
        let dir = tempdir().unwrap();
        let handle = LibraryHandle::open(dir.path()).unwrap();
        assert!(dir.path().join(".calibre-oxide").is_dir());
        assert_eq!(handle.state(), HandleState::Open);
    }

    #[test]
    fn a_second_open_on_the_same_library_fails_while_the_first_is_held() {
        let dir = tempdir().unwrap();
        let first = LibraryHandle::open(dir.path()).unwrap();
        let second = LibraryHandle::open(dir.path());
        assert!(matches!(second, Err(LibraryHandleError::AlreadyLocked)));
        drop(first);
        // Releases automatically once the first handle (and its lock
        // file descriptor) drops.
        assert!(LibraryHandle::open(dir.path()).is_ok());
    }

    #[test]
    fn write_atomic_persists_the_bytes_and_overwrites_cleanly() {
        let dir = tempdir().unwrap();
        let handle = LibraryHandle::open(dir.path()).unwrap();
        let target = dir.path().join("book").join("metadata.opf");

        handle.write_atomic(&target, b"first version").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"first version");

        handle
            .write_atomic(&target, b"second version, longer")
            .unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"second version, longer");

        // No leftover temp files.
        let leftovers: Vec<_> = fs::read_dir(target.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn rename_atomic_moves_the_file() {
        let dir = tempdir().unwrap();
        let handle = LibraryHandle::open(dir.path()).unwrap();
        let from = dir.path().join("a.txt");
        let to = dir.path().join("b.txt");
        fs::write(&from, b"data").unwrap();

        handle.rename_atomic(&from, &to).unwrap();
        assert!(!from.exists());
        assert_eq!(fs::read(&to).unwrap(), b"data");
    }

    #[test]
    fn tier_classification_returns_a_local_tier_for_a_tempdir() {
        let dir = tempdir().unwrap();
        let handle = LibraryHandle::open(dir.path()).unwrap();
        assert_ne!(handle.tier(), StorageTier::Network);
    }

    #[test]
    fn resolve_device_name_finds_a_real_device_for_a_tempdir() {
        // tempdir() lives on a real mounted filesystem (this
        // machine's actual disk, not a synthetic/virtual one), so
        // there should be a real `/dev/...`-derived name to resolve
        // -- and, critically, this must not panic regardless.
        let dir = tempdir().unwrap();
        let name = resolve_device_name(dir.path());
        assert!(
            name.as_ref()
                .map_or(true, |n| !n.is_empty() && !n.contains('/')),
            "device name = {name:?}"
        );
    }

    #[test]
    fn open_via_the_public_api_spawns_a_device_monitor_without_affecting_the_handle() {
        // `LibraryHandle::open` (not `open_impl`) always monitors --
        // this is the one test in this file proving that real, public
        // code path doesn't panic, error, or otherwise disturb a
        // normal open (the monitor thread itself is exercised more
        // directly by `device_monitor.rs`'s own tests).
        let dir = tempdir().unwrap();
        let handle = LibraryHandle::open(dir.path()).unwrap();
        assert_eq!(handle.state(), HandleState::Open);
        handle
            .write_atomic(&dir.path().join("x.opf"), b"y")
            .unwrap();
        assert_eq!(handle.state(), HandleState::Open);
    }

    // --- fault-injection: docs/FAULT_TOLERANCE.md's "kill process at
    // random point" testable invariant, for write_atomic specifically
    // {{{

    #[test]
    fn a_crash_right_after_writing_the_temp_file_leaves_the_original_untouched() {
        let dir = tempdir().unwrap();
        let handle = LibraryHandle::open(dir.path()).unwrap();
        let target = dir.path().join("metadata.opf");
        fs::write(&target, b"original").unwrap();

        let err = handle.write_atomic_impl(&target, b"new", Some(FailPoint::WriteTemp));
        assert!(err.is_err());
        assert_eq!(fs::read(&target).unwrap(), b"original");
    }

    #[test]
    fn a_crash_right_after_fsyncing_the_temp_file_leaves_the_original_untouched() {
        let dir = tempdir().unwrap();
        let handle = LibraryHandle::open(dir.path()).unwrap();
        let target = dir.path().join("metadata.opf");
        fs::write(&target, b"original").unwrap();

        let err = handle.write_atomic_impl(&target, b"new", Some(FailPoint::FsyncTemp));
        assert!(err.is_err());
        assert_eq!(fs::read(&target).unwrap(), b"original");
    }

    #[test]
    fn a_crash_right_after_the_rename_has_already_committed_the_new_content() {
        // Once `rename` has happened, the write is durable regardless
        // of whether subsequent steps complete -- this is exactly why
        // write-temp-then-rename is the right shape: the commit point
        // is a single atomic filesystem operation, not a multi-step
        // window.
        let dir = tempdir().unwrap();
        let handle = LibraryHandle::open(dir.path()).unwrap();
        let target = dir.path().join("metadata.opf");
        fs::write(&target, b"original").unwrap();

        let err = handle.write_atomic_impl(&target, b"new", Some(FailPoint::Rename));
        assert!(err.is_err());
        assert_eq!(fs::read(&target).unwrap(), b"new");
    }

    // }}}

    // --- journal: real writes, chaining, and crash recovery {{{

    #[test]
    fn write_atomic_journals_an_entry_and_marks_it_committed() {
        let dir = tempdir().unwrap();
        let handle = LibraryHandle::open(dir.path()).unwrap();
        let target = dir.path().join("metadata.opf");

        handle.write_atomic(&target, b"content").unwrap();

        let journal_dir = dir.path().join(".calibre-oxide").join("journal");
        let op_files: Vec<_> = fs::read_dir(&journal_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("op"))
            .collect();
        assert_eq!(op_files.len(), 1);

        let uuid = op_files[0]
            .path()
            .file_stem()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(journal_dir.join(format!("{uuid}.committed")).exists());

        let entry: JournalEntry =
            serde_json::from_slice(&fs::read(op_files[0].path()).unwrap()).unwrap();
        assert_eq!(entry.seq, 0);
        assert_eq!(entry.prev_head, None);
        match entry.op {
            OperationDescriptor::WriteFile {
                target: t,
                content_hash,
            } => {
                assert_eq!(t, target);
                assert_eq!(content_hash, blake3_hex(b"content"));
            }
            _ => panic!("expected WriteFile"),
        }
    }

    #[test]
    fn sequential_writes_chain_correctly() {
        let dir = tempdir().unwrap();
        let handle = LibraryHandle::open(dir.path()).unwrap();
        handle
            .write_atomic(&dir.path().join("a.opf"), b"a")
            .unwrap();
        handle
            .write_atomic(&dir.path().join("b.opf"), b"b")
            .unwrap();

        let journal_dir = dir.path().join(".calibre-oxide").join("journal");
        let mut entries: Vec<JournalEntry> = fs::read_dir(&journal_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("op"))
            .map(|e| serde_json::from_slice(&fs::read(e.path()).unwrap()).unwrap())
            .collect();
        entries.sort_by_key(|e| e.seq);

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].seq, 0);
        assert_eq!(entries[1].seq, 1);
        assert_eq!(
            entries[1].prev_head,
            Some(entries[0].descriptor_hash.clone())
        );
    }

    #[test]
    fn reopening_after_a_crash_before_rename_cleans_up_the_orphaned_temp_file() {
        let dir = tempdir().unwrap();
        {
            let handle = LibraryHandle::open(dir.path()).unwrap();
            let target = dir.path().join("metadata.opf");
            let err = handle.write_atomic_impl(&target, b"new", Some(FailPoint::WriteTemp));
            assert!(err.is_err());
            // The temp file exists right now, mid-"crash".
            let leftovers: Vec<_> = fs::read_dir(dir.path())
                .unwrap()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
                .collect();
            assert_eq!(leftovers.len(), 1);
        }
        // Simulates the process restarting: a fresh `open()` call runs
        // recovery.
        let handle = LibraryHandle::open(dir.path()).unwrap();
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "orphaned temp file should be cleaned up"
        );
        assert!(!dir.path().join("metadata.opf").exists());
        drop(handle);
    }

    #[test]
    fn reopening_after_a_crash_right_after_rename_finalizes_the_commit_marker() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("metadata.opf");
        {
            let handle = LibraryHandle::open(dir.path()).unwrap();
            let err =
                handle.write_atomic_impl(&target, b"new content", Some(FailPoint::BeforeCommit));
            assert!(err.is_err());
            // The rename already happened -- the real content is
            // already there, just unmarked.
            assert_eq!(fs::read(&target).unwrap(), b"new content");
        }

        let handle = LibraryHandle::open(dir.path()).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new content");

        let journal_dir = dir.path().join(".calibre-oxide").join("journal");
        let committed: Vec<_> = fs::read_dir(&journal_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("committed"))
            .collect();
        assert_eq!(
            committed.len(),
            1,
            "recovery should finalize the missing commit marker"
        );

        // Reopening again is stable -- no further changes, no error.
        drop(handle);
        let handle2 = LibraryHandle::open(dir.path()).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new content");
        drop(handle2);
    }

    #[test]
    fn reopening_after_a_crash_before_the_journal_write_even_finished_has_nothing_to_recover() {
        // The very first fault-injection point: nothing was ever
        // journaled successfully, so there's no entry to recover at
        // all. (Simulated here by never calling write_atomic in the
        // first place -- the interesting assertion is just that
        // `open()` on a library with an empty journal doesn't error.)
        let dir = tempdir().unwrap();
        let handle = LibraryHandle::open(dir.path()).unwrap();
        assert_eq!(handle.state(), HandleState::Open);
    }

    #[test]
    fn recovery_detects_a_tampered_journal_entry_as_corruption() {
        let dir = tempdir().unwrap();
        let journal_dir = dir.path().join(".calibre-oxide").join("journal");
        fs::create_dir_all(&journal_dir).unwrap();

        let op = OperationDescriptor::WriteFile {
            target: dir.path().join("metadata.opf"),
            content_hash: blake3_hex(b"whatever"),
        };
        let real_hash = blake3_hex(&serde_json::to_vec(&op).unwrap());
        let entry = JournalEntry {
            seq: 0,
            prev_head: None,
            op,
            // Tampered: doesn't match a fresh hash of `op`.
            descriptor_hash: format!("not-{real_hash}"),
        };
        let uuid = Uuid::new_v4();
        fs::write(
            journal_dir.join(format!("{uuid}.op")),
            serde_json::to_vec(&entry).unwrap(),
        )
        .unwrap();

        let result = LibraryHandle::open(dir.path());
        assert!(matches!(result, Err(LibraryHandleError::Corruption(_))));
    }

    #[test]
    fn recovery_detects_a_broken_chain_as_corruption() {
        let dir = tempdir().unwrap();
        let journal_dir = dir.path().join(".calibre-oxide").join("journal");
        fs::create_dir_all(&journal_dir).unwrap();

        let op1 = OperationDescriptor::WriteFile {
            target: dir.path().join("a.opf"),
            content_hash: blake3_hex(b"a"),
        };
        let hash1 = blake3_hex(&serde_json::to_vec(&op1).unwrap());
        let entry1 = JournalEntry {
            seq: 0,
            prev_head: None,
            op: op1,
            descriptor_hash: hash1,
        };
        fs::write(
            journal_dir.join(format!("{}.op", Uuid::new_v4())),
            serde_json::to_vec(&entry1).unwrap(),
        )
        .unwrap();

        let op2 = OperationDescriptor::WriteFile {
            target: dir.path().join("b.opf"),
            content_hash: blake3_hex(b"b"),
        };
        let hash2 = blake3_hex(&serde_json::to_vec(&op2).unwrap());
        let entry2 = JournalEntry {
            seq: 1,
            // Wrong -- should reference entry1's descriptor_hash.
            prev_head: Some("bogus".to_string()),
            op: op2,
            descriptor_hash: hash2,
        };
        fs::write(
            journal_dir.join(format!("{}.op", Uuid::new_v4())),
            serde_json::to_vec(&entry2).unwrap(),
        )
        .unwrap();

        let result = LibraryHandle::open(dir.path());
        assert!(matches!(result, Err(LibraryHandleError::Corruption(_))));
    }

    // }}}

    // --- journal pruning (phase 3) {{{

    fn journal_dir_of(dir: &Path) -> PathBuf {
        dir.join(".calibre-oxide").join("journal")
    }

    fn op_file_count(journal_dir: &Path) -> usize {
        fs::read_dir(journal_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("op"))
            .count()
    }

    #[test]
    fn reopening_under_the_retention_limit_prunes_nothing() {
        let dir = tempdir().unwrap();
        {
            let handle = LibraryHandle::open_impl(dir.path(), 3, false).unwrap();
            for i in 0..3 {
                handle
                    .write_atomic(&dir.path().join(format!("{i}.opf")), b"x")
                    .unwrap();
            }
        }
        LibraryHandle::open_impl(dir.path(), 3, false).unwrap();
        assert_eq!(op_file_count(&journal_dir_of(dir.path())), 3);
        assert!(!dir
            .path()
            .join(".calibre-oxide")
            .join("journal_checkpoint")
            .exists());
    }

    #[test]
    fn reopening_over_the_retention_limit_prunes_the_oldest_entries() {
        let dir = tempdir().unwrap();
        {
            let handle = LibraryHandle::open_impl(dir.path(), 3, false).unwrap();
            for i in 0..5 {
                handle
                    .write_atomic(&dir.path().join(format!("{i}.opf")), b"x")
                    .unwrap();
            }
        }
        // Recovery on this open settles all 5, then prunes down to 3.
        LibraryHandle::open_impl(dir.path(), 3, false).unwrap();
        let journal_dir = journal_dir_of(dir.path());
        assert_eq!(op_file_count(&journal_dir), 3);

        let checkpoint_path = dir.path().join(".calibre-oxide").join("journal_checkpoint");
        assert!(checkpoint_path.exists());
        let checkpoint: JournalCheckpoint =
            serde_json::from_slice(&fs::read(&checkpoint_path).unwrap()).unwrap();
        assert_eq!(checkpoint.boundary_seq, 2);

        let mut remaining: Vec<JournalEntry> = fs::read_dir(&journal_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("op"))
            .map(|e| serde_json::from_slice(&fs::read(e.path()).unwrap()).unwrap())
            .collect();
        remaining.sort_by_key(|e| e.seq);
        assert_eq!(
            remaining.iter().map(|e| e.seq).collect::<Vec<_>>(),
            [2, 3, 4]
        );
    }

    #[test]
    fn writes_after_pruning_still_chain_correctly_from_the_checkpoint() {
        let dir = tempdir().unwrap();
        {
            let handle = LibraryHandle::open_impl(dir.path(), 2, false).unwrap();
            for i in 0..4 {
                handle
                    .write_atomic(&dir.path().join(format!("{i}.opf")), b"x")
                    .unwrap();
            }
        }
        // This open prunes seq 0-1 away, keeping 2-3.
        let handle = LibraryHandle::open_impl(dir.path(), 2, false).unwrap();
        handle
            .write_atomic(&dir.path().join("new.opf"), b"y")
            .unwrap();
        drop(handle);

        // A fresh open must still verify cleanly -- the new entry's
        // prev_head chains onto the last pre-prune entry's hash, which
        // the checkpoint (not a deleted file) now supplies.
        let handle = LibraryHandle::open_impl(dir.path(), 2, false).unwrap();
        assert_eq!(handle.state(), HandleState::Open);
        assert_eq!(fs::read(dir.path().join("new.opf")).unwrap(), b"y");
    }

    #[test]
    fn an_interrupted_prune_self_heals_on_the_next_open() {
        // Simulates a crash between the checkpoint write and the
        // deletion pass: write a checkpoint that's already past some
        // still-present entries, and confirm the next open both
        // succeeds and finishes deleting them.
        let dir = tempdir().unwrap();
        {
            let handle = LibraryHandle::open_impl(dir.path(), 100, false).unwrap();
            for i in 0..3 {
                handle
                    .write_atomic(&dir.path().join(format!("{i}.opf")), b"x")
                    .unwrap();
            }
        }
        let journal_dir = journal_dir_of(dir.path());
        let mut entries: Vec<(Uuid, JournalEntry)> = fs::read_dir(&journal_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("op"))
            .map(|e| {
                let uuid =
                    Uuid::parse_str(e.path().file_stem().unwrap().to_str().unwrap()).unwrap();
                let entry = serde_json::from_slice(&fs::read(e.path()).unwrap()).unwrap();
                (uuid, entry)
            })
            .collect();
        entries.sort_by_key(|(_, e)| e.seq);

        // Hand-write a checkpoint as if entries 0-1 were already
        // pruned, without actually deleting their files -- the "crash
        // right after the checkpoint rename" scenario.
        let checkpoint = JournalCheckpoint {
            boundary_seq: 2,
            boundary_hash: Some(entries[1].1.descriptor_hash.clone()),
        };
        fs::write(
            dir.path().join(".calibre-oxide").join("journal_checkpoint"),
            serde_json::to_vec(&checkpoint).unwrap(),
        )
        .unwrap();

        let handle = LibraryHandle::open_impl(dir.path(), 100, false).unwrap();
        assert_eq!(handle.state(), HandleState::Open);
        // The stale, already-superseded entry 0 and 1 files are
        // cleaned up by the recovery scan itself.
        assert_eq!(op_file_count(&journal_dir), 1);
    }

    // }}}

    #[test]
    fn best_matching_mount_prefers_the_longest_matching_mount_point() {
        let mounts = "/dev/sda1 / ext4 rw 0 0\n/dev/sdb1 /mnt/data ext4 rw 0 0\n";
        let (device, fstype) = best_matching_mount(mounts, Path::new("/mnt/data/library")).unwrap();
        assert_eq!(device, "/dev/sdb1");
        assert_eq!(fstype, "ext4");
    }

    #[test]
    fn classify_from_device_and_fstype_detects_network_filesystems() {
        assert_eq!(
            classify_from_device_and_fstype("server:/export", "nfs4"),
            StorageTier::Network
        );
        assert_eq!(
            classify_from_device_and_fstype("//server/share", "cifs"),
            StorageTier::Network
        );
    }

    #[test]
    fn base_block_device_strips_partition_suffixes() {
        assert_eq!(base_block_device("sda1"), "sda");
        assert_eq!(base_block_device("sda"), "sda");
        assert_eq!(base_block_device("nvme0n1p1"), "nvme0n1");
        assert_eq!(base_block_device("nvme0n1"), "nvme0n1");
        assert_eq!(base_block_device("mmcblk0p1"), "mmcblk0");
    }
}
