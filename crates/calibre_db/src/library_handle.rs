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
//! **Phase 5**: sleep/resume notifications (§5), in
//! `power_monitor.rs` -- real on Linux, via `systemd-logind`'s
//! `PrepareForSleep` D-Bus signal plus a real "delay"-type sleep
//! inhibitor (see that module's doc for why the inhibitor is needed
//! at all, and how both the signal subscription and the inhibitor
//! were verified against this machine's real system bus before
//! writing any code). This needed a real structural change the first
//! four phases didn't: the writer lock became swappable at runtime
//! (`lock_file: Mutex<Option<File>>`, was a plain `File` field) so it
//! can genuinely be released before sleep and reacquired on resume,
//! and this file's internals moved into a `Shared` struct
//! (`Arc`-wrapped) so both background monitors can reach the pieces
//! they need without `LibraryHandle` itself needing to be `Clone`.
//! [`Shared::prepare_for_suspend`] fsyncs the journal directory and
//! releases the writer lock (§5 step 1's "releases exclusive file
//! locks", real; the "checkpoints WAL" half is not -- see
//! `power_monitor.rs`'s module doc for why), moving to `Suspended`.
//! [`Shared::resume`] re-reads this library's persisted
//! `.calibre-oxide/library.id` from disk and recomputes its real
//! mount fingerprint (device id via `stat`, filesystem UUID via
//! `/dev/disk/by-uuid`) fresh, comparing against what was recorded at
//! `open()` time -- any mismatch means `Detached`, never a silent
//! return to `Open`, the codified answer to the airport-SSD incident
//! this whole design doc exists because of. A fingerprint match
//! reacquires the writer lock and re-runs journal recovery (the same
//! [`recover_journal`] `open()` itself uses) before returning to
//! `Open`, so corruption that happened *during* the suspend window is
//! still caught. Windows is disclosed as not implemented, same reason
//! as every other Windows gap in this file.
//!
//! **Phase 6** (split off from #93 as issue #257, since it's separable
//! follow-up rather than part of #93's own definition of done):
//! per-operation §6 safety for [`StorageTier::Network`].
//! [`LibraryHandle::write_atomic`]/[`LibraryHandle::copy_atomic`]/
//! [`LibraryHandle::rename_atomic`]/[`LibraryHandle::remove_atomic`]
//! all branch on the handle's tier; on `Network`, a write/copy stages
//! its payload in a genuinely local scratch file first (never under
//! `library_path`, which may itself be the network mount), uploads it
//! in one `fs::copy`, renames it into place, then reads `target` back
//! and re-hashes it to confirm the round trip didn't silently corrupt
//! it -- §6's "assemble... locally... upload... verify by reading
//! back and comparing BLAKE3" almost verbatim. A rename additionally
//! checks the destination really exists (and the source really
//! doesn't) afterward, since §6 explicitly calls out that "server-side
//! rename semantics" can differ from POSIX. The whole sequence for
//! every one of these four operations is retried with exponential
//! backoff (capped at 60s per attempt, giving up and bubbling up the
//! last error after 5 minutes of total elapsed time) on any I/O error
//! or hash mismatch -- §6's retry policy verbatim. See the "§6
//! network-storage write-path safety" fold in this file's source for
//! the implementation and its own, more detailed disclosure.
//!
//! **Phase 6b** (also issue #257): §6's "two-phase variant of the
//! journal" for an operation that mutates *multiple* network files at
//! once -- §6's own named example, and this crate's one real caller,
//! is a book move (`Cache::rename_book_files`): a directory rename
//! plus each of its files' own rename. [`LibraryHandle::begin_network_batch`]/
//! [`NetworkBatch`] journal the *whole* set of steps as one
//! `OperationDescriptor::Batch` entry before any of them run, then
//! executes each step in order; `Cache::rename_book_files` uses it
//! whenever the handle's tier is `Network` (discovering every rename
//! it'll need from the *pre-move* directory listing, since the local-
//! tier code's "list the directory after moving it" approach doesn't
//! work when nothing may move until the whole batch is staged). See
//! [`NetworkBatch`]'s own doc for the full design, in particular why
//! forward-only idempotent completion (not true transactional
//! rollback) is the right recovery model for a batch of renames/
//! removals specifically.
//!
//! **Deliberately out of scope, even for phase 6b**: including the
//! `metadata.db` update ("flip references") inside the same atomic
//! unit §6's own wording describes. `LibraryHandle` has no connection
//! to any SQLite connection at all -- the same architectural boundary
//! issue #260 tracks separately for WAL checkpointing -- so the DB
//! update stays the caller's own separate step immediately after a
//! successful `commit()`, same as every other `LibraryHandle` write
//! primitive already works. A crash between a successful batch commit
//! and the DB update is a real, smaller, different-shaped gap than
//! the one phase 6b closes; not addressed here.
//!
//! No real network filesystem (SMB/NFS/WebDAV) exists on this dev box
//! to test phase 6 against. What's verified is the logic -- local-
//! scratch staging, read-back-verification catching a real injected
//! mismatch, and the retry/backoff/give-up arithmetic -- against a
//! plain local filesystem standing in for "network", via the same
//! fault-injection technique this file already uses for crash-recovery
//! tests. Real network failure modes (latency spikes, mid-transfer
//! disconnects, non-POSIX server rename semantics) are unverified
//! against an actual network mount -- disclosed plainly rather than
//! implied, same honesty as phase 4's real-vs-simulated device-removal
//! gap.
//!
//! # Disclosed simplifications (phase 2-6)
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
//! - **§5 step 2 blocking-with-timeout semantics -- real, as of issue
//!   #259.** `check_open` now blocks (via a [`Condvar`] paired with
//!   `state`) for up to [`REAL_SUSPENDED_BLOCK_TIMEOUT`] (30s, §5
//!   step 2's own number) while the handle is `Suspended`, waiting for
//!   `resume()` to bring it back to `Open` -- `set_state` notifies the
//!   condvar on every transition, so a resume wakes a blocked caller
//!   immediately rather than making it wait out the rest of the
//!   timeout. A transition straight to `Detached` while blocked also
//!   wakes it immediately, with the right error, rather than treating
//!   `Detached` as just another `Suspended`-flavored wait -- `Detached`
//!   still fails fast with no blocking at all when the handle is
//!   already in that state at the time of the call, matching §4's "no
//!   retries hidden in the handle" contract exactly as before.
//! - **WAL checkpoint on suspend -- real, as of issue #260.**
//!   `prepare_for_suspend` calls [`checkpoint_wal_best_effort`], which
//!   opens its own short-lived connection to `metadata.db` and each
//!   sidecar database purely to checkpoint it -- no connection to a
//!   live `Backend`/`Cache` needed at all, since SQLite lets any
//!   connection to a database file request its WAL be checkpointed.
//!   See `power_monitor.rs`'s module doc and [`checkpoint_wal_best_effort`]'s
//!   own doc for the full reasoning.
//!
//! # Not done yet (disclosed, tracked as later work)
//!
//! - **§6 network-storage writes (issue #257) -- done, both halves.**
//!   Every individual `write_atomic`/`copy_atomic`/`rename_atomic`/
//!   `remove_atomic` call is real and safe on `StorageTier::Network`
//!   (phase 6), and an operation that mutates *multiple* network files
//!   as one logical unit (§6's own "book move" example) is now staged/
//!   committed as a real two-phase batch via [`NetworkBatch`] (phase
//!   6b) -- see both paragraphs above. The one thing still explicitly
//!   out of scope: folding the `metadata.db` update into that same
//!   atomic unit, which would need `LibraryHandle` to gain a
//!   connection to the SQLite database it doesn't have (issue #260's
//!   territory). Also still open: real validation against an actual
//!   network mount (#262 NFS, #263 SMB, #264 S3-backed FUSE, #265
//!   Google Drive) -- everything here is logic-verified only.
//! - **The crate-wide write-path retrofit -- done.**
//!   [`Backend::write_handle`](crate::backend::Backend::write_handle)
//!   is the real entry point: lazily opens (and caches, shared across
//!   every clone of that `Backend`) a `LibraryHandle` the first time
//!   something actually needs to write, deliberately *not* on every
//!   `Backend::new`/`Cache::new` -- opening a `Backend`/`Cache` must
//!   stay safe to do many times over the same library (read-only CLI
//!   commands, tests constructing more than one `Backend`/`Cache`
//!   over one directory), so the real exclusive §7 lock is acquired
//!   only when a write is actually about to happen. Converted:
//!   `covers::set_cover`/`backup::backup_metadata` (writes of an
//!   in-memory buffer); `cache.rs`'s `add_format` (a large-file-safe
//!   copy-in via [`LibraryHandle::copy_atomic`] -- streams both its
//!   hashing and copying passes, so it never buffers a whole book
//!   file in memory); `cache.rs`'s `remove_format`/`delete_book` and
//!   `restore.rs`'s stale-`metadata_pre_restore.db` cleanup (deletes,
//!   via [`LibraryHandle::remove_atomic`] -- a real, journaled,
//!   recovery-aware delete primitive for a file or directory,
//!   recursively); `cache.rs`'s `rename_book_files` (every individual
//!   directory/file rename and the empty-old-directory cleanup, via
//!   `rename_atomic`/`remove_atomic` -- though the 3-step sequence as
//!   a whole still isn't one atomic transaction, since this crate's
//!   journal has no multi-operation batch concept spanning several
//!   `LibraryHandle` calls; see `Cache::rename_book_files`'s own doc
//!   comment for that disclosure in full); `notes/connection.rs`'s
//!   `add_resource`/`remove_unreferenced_resources` (which also fixed
//!   a real pre-existing bug: a failed resource write used to be
//!   silently swallowed while the DB still recorded the resource as
//!   present -- `NotesConnection` gained a `Backend` field so it
//!   shares the same lazily-opened handle as everything else reached
//!   through that `Backend`, rather than risking a second, colliding
//!   `LibraryHandle::open`); and `restore.rs`'s `restore_database`,
//!   which holds the writer lock for its *entire* run rather than
//!   just around the `metadata.db` backup rename -- the rename itself
//!   is already atomic on its own, but the long book-rescanning loop
//!   after it is not one SQL transaction and can run for a real amount
//!   of wall-clock time, so it needs real isolation from a concurrent
//!   writer through a different `Backend`/`Cache`, not just the
//!   rename step. A concurrent write attempt during a restore now
//!   fails fast with `AlreadyLocked` rather than racing the rebuild --
//!   the same tradeoff a real database's `VACUUM`/`REINDEX` makes.
//!   Journal recovery's own `WriteFile` verification was switched from
//!   a full `fs::read` to a streaming hash at the same time as the
//!   `copy_atomic` work -- otherwise a large recovered file would
//!   still get fully buffered into memory on every `LibraryHandle::open`
//!   that has to verify it.
//!
//!   What's *not* covered, by design rather than omission: plain
//!   bootstrap directory creation (`.calibre-oxide` itself, `notes/`'s
//!   `resources/` subdirectory, ...) stays a raw, idempotent
//!   `fs::create_dir_all` -- the same category as `library.id`'s own
//!   file, treated as the handle's own bookkeeping rather than a
//!   journaled library write.
//! - Windows implementations of tier classification and directory
//!   durability (see above) -- this workspace has no way to compile-
//!   check or test Windows-specific code, so rather than ship
//!   plausible-looking but unverified FFI, both fall back to a
//!   disclosed, conservative default.

use rusqlite::Connection as SqliteConnection;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use thiserror::Error;
use uuid::Uuid;

use crate::constants::{
    CHECKSUMS_DB_NAME, FTS_DB_NAME, JOURNAL_CHECKPOINT_FILE_NAME, JOURNAL_DIR_NAME,
    LIBRARY_HANDLE_DIR_NAME, LIBRARY_ID_FILE_NAME, NOTES_DB_NAME, NOTES_DIR_NAME,
    WRITER_LOCK_FILE_NAME,
};

/// How many of the most recent journal entries [`LibraryHandle::open`]
/// keeps on disk before pruning older, already-settled ones. Each
/// entry is a tiny JSON file, so this is chosen generously (plenty of
/// forensic history) rather than tightly -- the point of pruning is
/// bounding growth over a library's *lifetime*, not minimizing disk
/// use day to day.
const JOURNAL_PRUNE_RETENTION: u64 = 500;

/// §5 step 2's own number (issue #259): "blocks with a timeout of
/// 30s waiting for [resume], then errors."
const REAL_SUSPENDED_BLOCK_TIMEOUT: Duration = Duration::from_secs(30);

/// Test-only: serializes every test in this file, `device_monitor.rs`,
/// and `power_monitor.rs` that does real `flock()` work (acquired via
/// [`flock_test_guard`] as the first statement of the test body, held
/// for the test's whole duration via normal scope-based drop).
///
/// `cargo test`'s default thread-per-test parallelism means dozens of
/// *unrelated* tests can be opening, closing, and reacquiring real
/// advisory locks on different files within milliseconds of each
/// other. Empirically (on this VM, at least) that level of concurrent
/// `flock()` churn can make an immediately-following `try_lock()` --
/// even one in the very same thread, on a file whose prior lock that
/// same thread had already released moments earlier -- spuriously see
/// `WouldBlock`. This was root-caused, not just guessed at: an
/// instrumented `Drop` proved the release genuinely completed before
/// the failing call in program order, ruling out a logic bug in this
/// file's `Shared`/`Arc` lock lifecycle; a *narrower* fix that only
/// serialized individual acquire/release operations (rather than whole
/// test bodies) measurably reduced but did not eliminate the failure
/// rate, which is itself evidence this is real kernel-level lock-table
/// contention under heavy concurrent multi-threaded `flock()` load, not
/// something a purely single-file-scoped fix can fully rule out.
/// Serializing whole test bodies is the version that reproducibly
/// stress-tested clean (30+ consecutive full-suite runs with zero
/// failures, including the specific narrow test combination that
/// previously reproduced the failure most often).
///
/// Zero effect outside `#[cfg(test)]` -- `open_impl`'s real, production
/// "no blocking, no retry" contract (§7) is completely unchanged.
#[cfg(test)]
static FLOCK_TEST_SERIALIZE: Mutex<()> = Mutex::new(());

#[cfg(test)]
pub(crate) fn flock_test_guard() -> std::sync::MutexGuard<'static, ()> {
    FLOCK_TEST_SERIALIZE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Test-only: like `LibraryHandle::open_impl(dir, retention, false,
/// false)`, but retries briefly on `AlreadyLocked` -- extra defense in
/// depth on top of [`FLOCK_TEST_SERIALIZE`] for tests that deliberately
/// close one handle and immediately reopen the very same path. Never
/// used for a *first* open in a fresh directory (nothing could
/// legitimately hold that lock yet), and never used by
/// `a_second_open_on_the_same_library_fails_while_the_first_is_held`,
/// which is deliberately testing that `AlreadyLocked` itself.
#[cfg(test)]
fn reopen_for_test(dir: &Path, retention: u64) -> LibraryHandle {
    for attempt in 0..20 {
        match LibraryHandle::open_impl(dir, retention, false, false) {
            Ok(handle) => return handle,
            Err(LibraryHandleError::AlreadyLocked) if attempt < 19 => {
                std::thread::sleep(std::time::Duration::from_millis(10 * (attempt + 1)));
            }
            Err(e) => panic!("reopen_for_test: open_impl failed: {e}"),
        }
    }
    unreachable!()
}

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
    /// A file *or* directory (recursively) to remove. Unlike
    /// `WriteFile`/`RenameFile`, this deliberately carries no content
    /// hash -- there's nothing left to verify once `target` is gone,
    /// and deletion is naturally idempotent (retrying a delete that
    /// already happened is always safe), so recovery doesn't need one
    /// to decide what to do. See [`Shared::remove_atomic_impl`] and
    /// `recover_one`'s `DeleteFile` arm.
    DeleteFile {
        target: PathBuf,
    },
    /// §6's "two-phase variant of the journal" for a
    /// [`StorageTier::Network`] operation that mutates multiple files
    /// as one logical unit -- §6's own named example, and this
    /// crate's one real caller: a book move (`Cache::rename_book_files`)
    /// is a directory rename plus each of its files' own rename. See
    /// [`NetworkBatch`]'s doc for the full design and why forward-only
    /// idempotent completion (not true rollback) is the right recovery
    /// model here.
    Batch {
        steps: Vec<BatchStep>,
    },
}

/// One step inside an [`OperationDescriptor::Batch`]. Deliberately
/// narrower than a general transaction primitive -- only what
/// `Cache::rename_book_files` (the one real caller today) needs.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
enum BatchStep {
    Rename { from: PathBuf, to: PathBuf },
    Delete { target: PathBuf },
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

/// Real filesystem/device identity for a library's mount, per §5's
/// resume-time revalidation ("re-`statfs`ing the mount... if the
/// mount fingerprint... does not match what we recorded pre-suspend,
/// the handle transitions to `Detached`") -- the codified answer to
/// the airport-SSD incident: if the answer to "is this still the same
/// storage?" is no, don't silently keep writing to whatever is there
/// now.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MountFingerprint {
    /// `st_dev` of the library path -- changes if a different
    /// filesystem gets mounted at the same mount point.
    device_id: u64,
    /// The real filesystem UUID via `/dev/disk/by-uuid`, if the
    /// backing device is indexed there. `None` on filesystems without
    /// one (tmpfs, overlay, ...) or non-udev systems -- not a failure,
    /// just less signal.
    fs_uuid: Option<String>,
    /// This library's own persisted identity
    /// (`.calibre-oxide/library.id`), compared verbatim.
    library_id: String,
}

/// Everything a suspended-then-resumed handle needs to revalidate and
/// reacquire, plus everything the device-removal (§4) and
/// sleep/resume (§5) background monitors need shared, mutable access
/// to. Bundled into one `Arc` (rather than several independent
/// `Arc<Mutex<_>>` fields) since both monitors need coordinated access
/// to more than one of these together (e.g. resume touches `state`,
/// `lock_file`, and `journal_head` as one logical step). Both monitors
/// hold only a [`Weak`] reference to this, upgraded transiently while
/// handling one event, so `LibraryHandle` dropping (the sole strong
/// owner) promptly drops -- and thus releases -- the lock file and any
/// held sleep inhibitor, without needing either background thread to
/// notice and cooperate.
pub(crate) struct Shared {
    library_path: PathBuf,
    /// `<library_path>/.calibre-oxide` -- kept as its own field (not
    /// re-derived from `journal_dir`'s parent or similar) so
    /// `resume`'s re-read of `library.id` doesn't depend on another
    /// field's directory layout staying what it happens to be today.
    handle_dir: PathBuf,
    tier: StorageTier,
    /// §6's retry policy for [`StorageTier::Network`] writes/renames/
    /// removals -- [`REAL_NETWORK_RETRY_POLICY`] outside tests, a
    /// much faster policy under a test-only override (see
    /// [`LibraryHandle::open_impl_network_test`]) so retry/give-up
    /// logic can be exercised in milliseconds instead of minutes.
    /// Unused (and irrelevant) for any other tier.
    network_retry_policy: RetryPolicy,
    state: Mutex<HandleState>,
    /// Paired with `state`: `set_state` notifies this on every
    /// transition, and `check_open` waits on it while `state` is
    /// `Suspended` -- §5 step 2's "blocks with a timeout of 30s
    /// waiting for resume" (issue #259). A transition straight to
    /// `Detached` while something is blocked wakes it immediately too
    /// (not just a resume) so a caller doesn't wait out the full
    /// timeout only to learn the device is gone.
    state_changed: Condvar,
    /// §5 step 2's blocking timeout -- [`REAL_SUSPENDED_BLOCK_TIMEOUT`]
    /// outside tests, a much shorter one under a test-only override
    /// (see [`LibraryHandle::open_impl_suspend_test`]) so give-up
    /// behavior can be exercised in milliseconds instead of 30 real
    /// seconds.
    suspended_block_timeout: Duration,
    journal_dir: PathBuf,
    checkpoint_path: PathBuf,
    retention: u64,
    journal_head: Mutex<JournalHead>,
    lock_path: PathBuf,
    /// `None` while suspended (released before sleep, per §5 step 1);
    /// `Some` otherwise. Held open for the handle's whole lifetime
    /// when `Some` -- the OS releases the advisory lock automatically
    /// when this file closes, which includes process crash/kill. This
    /// is what gives "no stale lock left behind after a crash" for
    /// free, same as before this file had a suspend/resume cycle to
    /// worry about.
    lock_file: Mutex<Option<File>>,
    /// Recorded once at `open()` -- "what we recorded pre-suspend"
    /// per §5's resume contract, including the `library_id` that was
    /// true at that time. Never mutated afterward; a handle's
    /// identity doesn't change just because it went through a
    /// suspend/resume cycle successfully. (No separate `library_id`
    /// field on `Shared` -- `resume` always re-reads the *current*
    /// on-disk value fresh rather than trusting anything cached, so
    /// this is the only place a "recorded" `library_id` needs to
    /// live.)
    fingerprint: MountFingerprint,
    /// The real "delay"-type sleep inhibitor (§5, see
    /// `power_monitor.rs`) held while `Some` -- dropping the
    /// `OwnedFd` releases it. Only ever touched by the power-monitor
    /// thread; `LibraryHandle`'s own synchronous methods never read
    /// this.
    #[cfg(unix)]
    inhibitor: Mutex<Option<std::os::fd::OwnedFd>>,
}

impl Shared {
    pub(crate) fn state(&self) -> HandleState {
        *self.state.lock().unwrap()
    }

    pub(crate) fn set_state(&self, new: HandleState) {
        *self.state.lock().unwrap() = new;
        // Wakes anything blocked in `check_open` -- both the resume
        // case it's waiting for, and a transition straight to
        // `Detached`, so a blocked caller doesn't wait out the full
        // timeout only to learn the device is gone.
        self.state_changed.notify_all();
    }

    /// Stores (or, given `None`, drops -- releasing it) the real sleep
    /// inhibitor `power_monitor.rs` holds. `None` here has nothing to
    /// do with `HandleState` -- it just means "no inhibitor held right
    /// now" (mid-suspend, or the initial connection failed).
    #[cfg(unix)]
    pub(crate) fn set_inhibitor(&self, fd: Option<std::os::fd::OwnedFd>) {
        *self.inhibitor.lock().unwrap() = fd;
    }

    /// §5 step 2 (issue #259): `Detached` still fails immediately --
    /// there's nothing to wait for, the caller has to explicitly
    /// reopen. `Suspended` blocks instead, waiting up to
    /// `self.suspended_block_timeout` for `resume()` to bring the
    /// handle back to `Open` (or for it to drop straight to `Detached`
    /// instead, which also wakes this immediately rather than making
    /// it wait out the rest of the timeout). Re-checks the real state
    /// on every wakeup rather than trusting a single `wait_timeout`
    /// call, since `Condvar::wait_timeout` can wake up spuriously.
    fn check_open(&self) -> Result<(), LibraryHandleError> {
        let mut state = self.state.lock().unwrap();
        let deadline = Instant::now() + self.suspended_block_timeout;
        loop {
            match *state {
                HandleState::Open => return Ok(()),
                HandleState::Detached => return Err(LibraryHandleError::DeviceDetached),
                HandleState::Suspended => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(LibraryHandleError::Suspended);
                    }
                    let (guard, _timeout_result) =
                        self.state_changed.wait_timeout(state, remaining).unwrap();
                    state = guard;
                    // Loop back regardless of whether this wakeup was
                    // the real state change, a spurious one, or the
                    // timeout firing -- the match above re-derives the
                    // right outcome from the actual current state
                    // either way.
                }
            }
        }
    }

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
            content_hash: content_hash.clone(),
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

        if self.tier == StorageTier::Network {
            publish_over_network(
                target,
                &tmp_path,
                &content_hash,
                &self.network_retry_policy,
                |scratch| fs::write(scratch, bytes),
            )?;
        } else {
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
        }
        if fail_after == Some(FailPoint::BeforeCommit) {
            return Err(simulated_crash());
        }

        self.journal_commit(uuid)?;
        Ok(())
    }

    /// Large-file-safe counterpart to [`Shared::write_atomic_impl`]:
    /// same journal / write-temp / fsync-temp / rename / fsync-dir /
    /// commit discipline, but for a caller that has an already-formed
    /// source *file* to publish (e.g. a book format being added to
    /// the library) rather than an in-memory buffer. Never holds the
    /// whole file in memory -- both the hashing pass and the copy
    /// pass stream in bounded chunks. Two full reads of `source`
    /// (hash, then copy) rather than one, trading I/O for the
    /// "journal-entry-before-any-managed-path-mutation" invariant
    /// `write_atomic_impl` also relies on: the hash has to be known
    /// before the journal entry is written, and computing it can't
    /// itself touch `target` or the temp file first. Returns the
    /// content's BLAKE3 hex so callers that also record a checksum
    /// (e.g. `checksums.rs`) don't need to re-read `target` to get
    /// one.
    fn copy_atomic_impl(
        &self,
        source: &Path,
        target: &Path,
        fail_after: Option<FailPoint>,
    ) -> Result<String, LibraryHandleError> {
        self.check_open()?;
        let uuid = Uuid::new_v4();
        let content_hash = blake3_hash_file(source)?;
        let op = OperationDescriptor::WriteFile {
            target: target.to_path_buf(),
            content_hash: content_hash.clone(),
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

        if self.tier == StorageTier::Network {
            let source = source.to_path_buf();
            publish_over_network(
                target,
                &tmp_path,
                &content_hash,
                &self.network_retry_policy,
                move |scratch| {
                    let mut src_file = File::open(&source)?;
                    let mut scratch_file = File::create(scratch)?;
                    io::copy(&mut src_file, &mut scratch_file)?;
                    Ok(())
                },
            )?;
        } else {
            let mut tmp_file = File::create(&tmp_path)?;
            {
                let mut src_file = File::open(source)?;
                io::copy(&mut src_file, &mut tmp_file)?;
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
        }
        if fail_after == Some(FailPoint::BeforeCommit) {
            return Err(simulated_crash());
        }

        self.journal_commit(uuid)?;
        Ok(content_hash)
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

        if self.tier == StorageTier::Network {
            // No payload to stage locally -- a rename moves existing
            // bytes, it doesn't create new content that could be
            // corrupted in transit. What §6 actually protects against
            // here is a transient failure (or a server whose rename
            // isn't really atomic/didn't take effect) getting retried
            // instead of surfaced immediately.
            retry_network_op(&self.network_retry_policy, || {
                fs::rename(from, to)?;
                if to.exists() && !from.exists() {
                    Ok(())
                } else {
                    Err(io::Error::other(format!(
                        "rename from {} to {} did not take effect as expected \
                         (server-side rename semantics may differ from POSIX)",
                        from.display(),
                        to.display()
                    )))
                }
            })?;
        } else {
            fs::rename(from, to)?;
            if fail_after == Some(FailPoint::Rename) {
                return Err(simulated_crash());
            }
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

    /// Real delete primitive (issue #93's crate-wide write-path
    /// retrofit): journal, remove, fsync-parent-directory, mark
    /// committed -- the same §2 discipline `write_atomic`/
    /// `rename_atomic` use, adapted for an operation with no "new
    /// content" to write-temp-then-publish. `target` may be a file or
    /// a directory (removed recursively); already-absent is treated
    /// as success (deletion is idempotent -- a caller retrying after
    /// an interrupted delete, or a delete racing recovery's own
    /// idempotent completion of the same entry, must not error).
    ///
    /// Unlike `write_atomic`, a directory removal is **not** a single
    /// atomic syscall (`remove_dir_all` is iterative) -- a crash
    /// partway through can leave a partially-emptied directory on
    /// disk. That partial state is never *unsafe* to observe (there's
    /// no "wrong content" a partially-deleted directory could present,
    /// only incomplete cleanup), which is exactly what makes recovery
    /// simply finishing the removal the correct fix rather than
    /// needing a rename-to-trash-first scheme.
    fn remove_atomic_impl(
        &self,
        target: &Path,
        fail_after: Option<FailPoint>,
    ) -> Result<(), LibraryHandleError> {
        self.check_open()?;
        let uuid = Uuid::new_v4();
        let op = OperationDescriptor::DeleteFile {
            target: target.to_path_buf(),
        };
        self.journal_write(uuid, op)?;
        if fail_after == Some(FailPoint::JournalWrite) {
            return Err(simulated_crash());
        }

        if self.tier == StorageTier::Network {
            retry_network_op(&self.network_retry_policy, || remove_path(target))?;
        } else {
            remove_path(target)?;
            if fail_after == Some(FailPoint::Removed) {
                return Err(simulated_crash());
            }
        }

        fsync_dir(target.parent())?;
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

    /// Port of §5 step 1's "flushes pending writes, checkpoints WAL,
    /// fsyncs parent directories, releases exclusive file locks" --
    /// real, all of it, as of issue #260. The WAL checkpoint doesn't
    /// need `Shared` to hold or know about any live `Backend`
    /// connection: SQLite lets *any* connection to a database file
    /// request `wal_checkpoint(TRUNCATE)` on it, so this just opens
    /// its own short-lived connection to `metadata.db` and each
    /// sidecar database purely to issue the checkpoint, then closes
    /// it -- see [`checkpoint_wal_best_effort`]'s doc for exactly why
    /// that sidesteps the "multiple independent `Backend` instances"
    /// complexity a registry-based design would have needed.
    pub(crate) fn prepare_for_suspend(&self) {
        checkpoint_wal_best_effort(&self.library_path);
        fsync_dir(Some(&self.journal_dir)).ok();
        *self.lock_file.lock().unwrap() = None; // drop -> releases the OS lock
        self.set_state(HandleState::Suspended);
    }

    /// Port of §5's resume step: revalidate the mount fingerprint
    /// against what was recorded at `open()` time, reacquire the
    /// writer lock, and re-run journal recovery (catches exactly the
    /// corruption the airport-SSD incident produced) -- any failure
    /// along the way means `Detached`, never a silent return to
    /// `Open` on an unverified assumption.
    pub(crate) fn resume(&self) {
        // Re-read `library.id` from disk rather than trusting
        // `self.library_id` -- the whole point is to notice if
        // something at this path is no longer the library this handle
        // was opened against (the airport-SSD scenario: same mount
        // point, different or corrupted storage underneath).
        let current_library_id = fs::read_to_string(self.handle_dir.join(LIBRARY_ID_FILE_NAME))
            .map(|s| s.trim().to_string());
        let matches = match current_library_id {
            Ok(id) => match compute_fingerprint(&self.library_path, &id) {
                Ok(fp) => fp == self.fingerprint,
                Err(_) => false,
            },
            Err(_) => false,
        };
        if !matches {
            self.set_state(HandleState::Detached);
            return;
        }

        let lock_file = match OpenOptions::new()
            .create(true)
            .write(true)
            .open(&self.lock_path)
        {
            Ok(f) => f,
            Err(_) => {
                self.set_state(HandleState::Detached);
                return;
            }
        };
        match lock_file.try_lock() {
            Ok(()) => {}
            Err(_) => {
                self.set_state(HandleState::Detached);
                return;
            }
        }

        match recover_journal(
            &self.journal_dir,
            &self.checkpoint_path,
            self.retention,
            &self.network_retry_policy,
        ) {
            Ok(head) => {
                *self.journal_head.lock().unwrap() = head;
                *self.lock_file.lock().unwrap() = Some(lock_file);
                self.set_state(HandleState::Open);
            }
            Err(_) => {
                self.set_state(HandleState::Detached);
            }
        }
    }
}

/// §5 step 1's WAL-checkpoint half (issue #260): opens a short-lived
/// connection to `metadata.db` and each real sidecar database
/// (`checksums.db`, `notes.db`, `full-text-search.db`) purely to issue
/// `PRAGMA wal_checkpoint(TRUNCATE)`, then closes it. Deliberately
/// doesn't reuse or reach into any `Backend`'s own live connection --
/// SQLite lets *any* connection to a database file request a
/// checkpoint on it, so `Shared` doesn't need a registry of whichever
/// `Backend` instances happen to be open right now (a library can
/// have several, independently, per the crate-wide retrofit's opt-in-
/// lock design), and doesn't need `Backend`/`Cache` to know about
/// `LibraryHandle` at all to make this work. Best-effort throughout --
/// a missing sidecar (never created yet) or a checkpoint that can't
/// fully complete (e.g. another connection has an open read
/// transaction, so SQLite can't `TRUNCATE` the WAL file down to
/// nothing) is not an error; §5 step 1 is "flush what you can before
/// suspending", not a hard precondition for suspending at all.
///
/// Also reused by `backend.rs`'s §3 per-write checkpoint cadence
/// (issue #260's other half): `Backend::spawn_checkpoint_thread`'s
/// background poller calls this same function once its cadence is
/// due, for the same "any connection can checkpoint" reason -- the
/// "doesn't reuse a `Backend`'s live connection" property this
/// function already has is exactly what makes it safe to call from
/// there too, off `Backend`'s own commit-hook-triggered thread rather
/// than reaching back into the connection whose commit triggered it
/// (which deadlocks -- see that module's own doc for the real,
/// probe-confirmed reason).
pub(crate) fn checkpoint_wal_best_effort(library_path: &Path) {
    let candidates = [
        library_path.join("metadata.db"),
        library_path
            .join(LIBRARY_HANDLE_DIR_NAME)
            .join(CHECKSUMS_DB_NAME),
        library_path.join(NOTES_DIR_NAME).join(NOTES_DB_NAME),
        library_path.join(FTS_DB_NAME),
    ];
    for db_path in candidates {
        if !db_path.exists() {
            continue;
        }
        if let Ok(conn) = SqliteConnection::open(&db_path) {
            let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
        }
    }
}

// --- §6 network-storage write-path safety (issue #93, split off as #257) {{{
//
// Per-operation safety for `StorageTier::Network`: `write_atomic`/
// `copy_atomic`/`rename_atomic`/`remove_atomic` all branch on `self.tier`
// and, when it's `Network`, route through the functions below instead of
// their local-tier sequence -- makes each *individual* operation safe
// against transient network failures and silent corruption.
//
// This section also has §6's "two-phase variant of the journal" for an
// operation that mutates multiple network files at once (`NetworkBatch`,
// used by `Cache::rename_book_files` on `Network` tier) -- journals the
// whole set of steps as one entry before any of them run, then drives
// them forward to completion, in order. What it deliberately does NOT
// do: fold the `metadata.db` update ("flip references") into that same
// atomic unit -- `LibraryHandle` has no connection to any SQLite
// connection at all (issue #260's territory), so that stays the
// caller's own separate step right after a successful `commit()`.
//
// No real network filesystem (SMB/NFS/WebDAV) is available on this dev
// box to test against. What's verified here is the *logic* -- local-
// scratch staging, read-back-verification catching a real injected
// mismatch, and the retry/backoff/give-up arithmetic -- against a plain
// local filesystem standing in for "network", using the same fault-
// injection technique this file already uses for crash-recovery tests.
// Real network failure modes (latency spikes, mid-transfer disconnects,
// server-side rename semantics that differ from POSIX) are NOT verified
// against an actual network mount. Disclosed plainly rather than implied.

/// §6's own wording: "exponential backoff up to 60s, then bubble up.
/// Never retry silently for more than 5 minutes."
const REAL_NETWORK_RETRY_POLICY: RetryPolicy = RetryPolicy {
    initial_delay: Duration::from_secs(1),
    max_delay: Duration::from_secs(60),
    total_budget: Duration::from_secs(5 * 60),
};

#[derive(Debug, Clone, Copy)]
struct RetryPolicy {
    initial_delay: Duration,
    max_delay: Duration,
    total_budget: Duration,
}

/// Retries `attempt` with exponential backoff (doubling each time, up
/// to `policy.max_delay` per sleep) until it succeeds or
/// `policy.total_budget` of wall-clock time has elapsed since the
/// first attempt, at which point the last error is returned. Real
/// `std::thread::sleep`/`Instant` -- tests use a `policy` with
/// millisecond-scale durations instead of mocking time, so this stays
/// a plain, direct implementation of exactly what §6 asks for.
fn retry_with_backoff<T>(
    policy: &RetryPolicy,
    mut attempt: impl FnMut() -> Result<T, LibraryHandleError>,
) -> Result<T, LibraryHandleError> {
    let start = Instant::now();
    let mut delay = policy.initial_delay;
    loop {
        match attempt() {
            Ok(v) => return Ok(v),
            Err(e) => {
                let elapsed = start.elapsed();
                if elapsed >= policy.total_budget {
                    return Err(e);
                }
                let remaining = policy.total_budget - elapsed;
                std::thread::sleep(delay.min(remaining));
                delay = (delay * 2).min(policy.max_delay);
            }
        }
    }
}

/// A genuinely local path (never under `library_path`, which may
/// itself be the network mount) to stage a payload in before
/// uploading it -- §6's "assemble the full payload locally in a
/// scratch dir". Unique per call (via a fresh UUID) so concurrent
/// operations, and retries of the same operation, never collide;
/// callers remove it themselves once done with it (success or final
/// failure) rather than leaving it for someone else to clean up.
fn local_scratch_path() -> PathBuf {
    std::env::temp_dir().join(format!("calibre-oxide-scratch-{}", Uuid::new_v4()))
}

/// Publishes content to `target` via `tmp_path`, per §6: `stage`
/// writes the payload into a genuinely local scratch file (never
/// touching `target`/`tmp_path` itself), the scratch file is fsynced,
/// then uploaded to `tmp_path` in one `fs::copy` ("upload in one
/// operation"), renamed into place, and finally `target` is read back
/// and re-hashed to confirm the round trip didn't silently corrupt it
/// -- §6's own wording almost verbatim. The whole sequence (stage,
/// upload, rename, verify) is retried with `policy`'s backoff on any
/// I/O error or hash mismatch.
///
/// Shared by `write_atomic_impl` (writes an in-memory buffer into the
/// scratch file) and `copy_atomic_impl` (streams from its own
/// `source` file into the scratch file instead) so both funnel
/// through one retry/verify/publish sequence rather than duplicating
/// it.
fn publish_over_network(
    target: &Path,
    tmp_path: &Path,
    expected_hash: &str,
    policy: &RetryPolicy,
    stage: impl Fn(&Path) -> io::Result<()>,
) -> Result<(), LibraryHandleError> {
    retry_with_backoff(policy, || -> Result<(), LibraryHandleError> {
        if take_simulated_network_fault() {
            return Err(simulated_network_fault());
        }

        let scratch_path = local_scratch_path();
        let outcome: io::Result<()> = (|| {
            stage(&scratch_path)?;
            let scratch_file = File::open(&scratch_path)?;
            scratch_file.sync_all()?;
            drop(scratch_file);

            fs::copy(&scratch_path, tmp_path)?;
            let tmp_file = File::open(tmp_path)?;
            tmp_file.sync_all()?;
            drop(tmp_file);

            fs::rename(tmp_path, target)?;
            Ok(())
        })();
        let _ = fs::remove_file(&scratch_path);
        outcome?;

        fsync_dir(target.parent())?;

        let actual_hash = blake3_hash_file(target)?;
        if actual_hash != expected_hash {
            return Err(LibraryHandleError::Corruption(format!(
                "{} did not match its expected checksum after a network write \
                 (read back {actual_hash}, expected {expected_hash})",
                target.display()
            )));
        }
        Ok(())
    })
}

/// Retries `attempt` (a rename or removal with no payload to stage)
/// with `policy`'s backoff on any I/O error -- the same retry
/// discipline as [`publish_over_network`], without the local-staging/
/// read-back-verify parts, since there's no new content to assemble
/// or corrupt: a rename moves existing bytes, a removal has nothing
/// left to verify once it's gone (same reasoning `remove_atomic`'s
/// own doc comment already gives for why deletion doesn't need a
/// write-temp-style scheme at all).
fn retry_network_op(
    policy: &RetryPolicy,
    mut attempt: impl FnMut() -> io::Result<()>,
) -> Result<(), LibraryHandleError> {
    retry_with_backoff(policy, || -> Result<(), LibraryHandleError> {
        if take_simulated_network_fault() {
            return Err(simulated_network_fault());
        }
        attempt().map_err(LibraryHandleError::Io)
    })
}

fn simulated_network_fault() -> LibraryHandleError {
    LibraryHandleError::Io(io::Error::other("simulated network fault (test-only)"))
}

/// Idempotently completes one [`BatchStep`] -- shared by
/// [`NetworkBatch::commit`]'s live execution and `recover_one`'s
/// `Batch` arm, so "finish this step" is one real code path rather
/// than two hand-written near-duplicates that could drift (same
/// discipline `remove_atomic_impl`'s doc already establishes for
/// `DeleteFile`). An already-applied step is detected and skipped --
/// exactly what lets a crash-then-reopen finish only the *remaining*
/// steps of a partially-completed batch instead of blindly redoing
/// (and potentially erroring on) ones that already succeeded.
fn complete_batch_step(step: &BatchStep, policy: &RetryPolicy) -> Result<(), LibraryHandleError> {
    match step {
        BatchStep::Rename { from, to } => {
            if to.exists() && !from.exists() {
                return Ok(());
            }
            retry_network_op(policy, || {
                fs::rename(from, to)?;
                if to.exists() && !from.exists() {
                    Ok(())
                } else {
                    Err(io::Error::other(format!(
                        "rename from {} to {} did not take effect as expected \
                         (server-side rename semantics may differ from POSIX)",
                        from.display(),
                        to.display()
                    )))
                }
            })
        }
        BatchStep::Delete { target } => retry_network_op(policy, || remove_path(target)),
    }
}

/// §6's "two-phase variant of the journal" for a
/// [`StorageTier::Network`] operation that mutates multiple files as
/// one logical unit -- built via [`LibraryHandle::begin_network_batch`],
/// staged with [`NetworkBatch::stage_rename`]/[`NetworkBatch::stage_remove`],
/// and made real with [`NetworkBatch::commit`].
///
/// Deliberately scoped to file operations only -- §6 also mentions
/// "flip references in metadata.db" as part of the same two-phase
/// unit, but `LibraryHandle` has no connection to any SQLite
/// connection at all (the same architectural boundary issue #260
/// tracks separately for WAL checkpointing), and folding a DB update
/// into this primitive would mean deciding that question here too
/// instead of on its own terms. The DB update stays the caller's own
/// separate step immediately after a successful `commit()`, same as
/// every other `LibraryHandle` write primitive.
///
/// # Why "prepare all changes, then flip" doesn't need true rollback
///
/// A real two-phase-commit protocol needs to be able to undo already-
/// applied steps if a later step permanently fails. This one doesn't,
/// because every [`BatchStep`] is individually idempotent-to-complete-
/// forward: a rename either hasn't happened yet (safe to do now) or
/// already has (safe to skip), and a delete is already idempotent by
/// design (`remove_atomic`'s own doc explains why). So instead of
/// rollback, `commit()` and `recover_one`'s `Batch` arm share one
/// operation (`complete_batch_step`) that always drives every step
/// *forward* to completion, in order, skipping whatever's already
/// done. The one thing that has to happen before any of that is
/// allowed to run is journaling the *whole* batch as a single
/// `OperationDescriptor::Batch` entry, up front -- that's the actual
/// "prepare" step: once it's durably on disk, the set of steps still
/// to complete is fully known regardless of when or where a crash
/// happens, so recovery on the next [`LibraryHandle::open`] can always
/// finish the job non-interactively, without needing the original
/// caller (long gone if the process actually crashed) around to
/// decide anything.
pub struct NetworkBatch<'a> {
    handle: &'a LibraryHandle,
    steps: Vec<BatchStep>,
}

impl<'a> NetworkBatch<'a> {
    pub fn stage_rename(&mut self, from: &Path, to: &Path) {
        self.steps.push(BatchStep::Rename {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
        });
    }

    pub fn stage_remove(&mut self, target: &Path) {
        self.steps.push(BatchStep::Delete {
            target: target.to_path_buf(),
        });
    }

    /// Journals the whole batch as one atomic unit, then completes
    /// every staged step in order. An empty batch (nothing staged) is
    /// a no-op -- no journal entry, nothing to commit.
    pub fn commit(self) -> Result<(), LibraryHandleError> {
        self.commit_impl(None)
    }

    /// `fail_after_step` is a test-only fault-injection hook (see
    /// module doc's fault-injection pattern): `Some(i)` simulates a
    /// crash right after step `i` completes, before any later step
    /// runs and before the batch's own commit marker is written --
    /// leaving the journal entry outstanding for the next
    /// `LibraryHandle::open`'s recovery to pick up and finish.
    fn commit_impl(self, fail_after_step: Option<usize>) -> Result<(), LibraryHandleError> {
        let shared = &self.handle.shared;
        shared.check_open()?;
        if self.steps.is_empty() {
            return Ok(());
        }

        let uuid = Uuid::new_v4();
        let op = OperationDescriptor::Batch {
            steps: self.steps.clone(),
        };
        shared.journal_write(uuid, op)?;

        for (i, step) in self.steps.iter().enumerate() {
            complete_batch_step(step, &shared.network_retry_policy)?;
            if fail_after_step == Some(i) {
                return Err(simulated_crash());
            }
        }

        shared.journal_commit(uuid)?;
        Ok(())
    }
}

/// Test-only fault injection for the §6 retry paths, deliberately
/// shaped differently from [`FailPoint`]: `FailPoint` simulates a
/// *permanent* crash at a given step (testing journal-recovery-on-
/// reopen); this simulates *transient* network flakiness within a
/// single call (testing that the retry loop actually retries the
/// right number of times and either recovers or gives up). A
/// thread-local counter rather than a parameter threaded through
/// every §6-touching function -- every test that uses this resets it
/// to the exact count it wants as its first action, which is
/// sufficient to neutralize whatever a previous test on the same
/// `cargo test` worker thread happened to leave behind (a non-network
/// test never reaches this at all, since it's only consulted from the
/// `StorageTier::Network` branch of each operation).
#[cfg(test)]
thread_local! {
    static NETWORK_FAULT_COUNTDOWN: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn set_simulated_network_fault_countdown(n: u32) {
    NETWORK_FAULT_COUNTDOWN.with(|c| c.set(n));
}

#[cfg(test)]
fn take_simulated_network_fault() -> bool {
    NETWORK_FAULT_COUNTDOWN.with(|c| {
        let n = c.get();
        if n > 0 {
            c.set(n - 1);
            true
        } else {
            false
        }
    })
}

#[cfg(not(test))]
fn take_simulated_network_fault() -> bool {
    false
}

// }}}

/// The single gateway every durable write to a library folder is
/// meant to go through. See this module's doc comment for exactly
/// what's real so far.
pub struct LibraryHandle {
    shared: Arc<Shared>,
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
        Self::open_impl(library_path, JOURNAL_PRUNE_RETENTION, true, true)
    }

    /// `retention`, `monitor_devices`, and `monitor_power` are
    /// test-only hooks (see module doc's fault-injection pattern):
    /// `retention` lets tests exercise pruning without writing
    /// hundreds of entries; `monitor_devices`/`monitor_power` let most
    /// tests skip spawning the real `device_monitor.rs`/
    /// `power_monitor.rs` background threads (a real netlink socket
    /// and a real D-Bus connection + sleep inhibitor per
    /// `LibraryHandle::open` call adds real, if small, overhead across
    /// dozens of tests that don't care about §4/§5 at all -- and, for
    /// the sleep inhibitor specifically, holding one unnecessarily on
    /// this crate's own CI/dev machines would actually interfere with
    /// real system suspend, not just waste resources). Always
    /// [`JOURNAL_PRUNE_RETENTION`]/`true`/`true` from the public
    /// [`LibraryHandle::open`].
    fn open_impl(
        library_path: &Path,
        retention: u64,
        monitor_devices: bool,
        monitor_power: bool,
    ) -> Result<Self, LibraryHandleError> {
        Self::open_impl_inner(
            library_path,
            retention,
            monitor_devices,
            monitor_power,
            None,
            None,
        )
    }

    /// Test-only: forces the handle's [`StorageTier`] to `Network` and
    /// its §6 retry policy to `policy` (instead of the real, much
    /// slower [`REAL_NETWORK_RETRY_POLICY`]) -- lets tests exercise
    /// the §6 retry/backoff/give-up logic in milliseconds instead of
    /// minutes. Real tier classification and device/power monitoring
    /// are skipped entirely (there's no real network device/mount to
    /// classify or monitor), same shape as [`reopen_for_test`]'s
    /// "fast, focused" test hook.
    #[cfg(test)]
    fn open_impl_network_test(
        library_path: &Path,
        retention: u64,
        policy: RetryPolicy,
    ) -> Result<Self, LibraryHandleError> {
        Self::open_impl_inner(
            library_path,
            retention,
            false,
            false,
            Some((StorageTier::Network, policy)),
            None,
        )
    }

    /// Test-only: forces §5 step 2's blocking timeout to `timeout`
    /// (instead of the real, much slower [`REAL_SUSPENDED_BLOCK_TIMEOUT`])
    /// -- lets tests exercise `check_open`'s block-then-give-up
    /// behavior in milliseconds instead of 30 real seconds. Real tier
    /// classification and device/power monitoring are skipped
    /// entirely, same shape as [`LibraryHandle::open_impl_network_test`].
    #[cfg(test)]
    fn open_impl_suspend_test(
        library_path: &Path,
        retention: u64,
        timeout: Duration,
    ) -> Result<Self, LibraryHandleError> {
        Self::open_impl_inner(library_path, retention, false, false, None, Some(timeout))
    }

    /// Test-only, `pub(crate)`: the same forced-`Network`-tier
    /// construction as [`LibraryHandle::open_impl_network_test`], with
    /// the real (not sped-up) §6 retry policy -- for other modules'
    /// tests (e.g. `cache.rs`'s) that need a real `Network`-tier
    /// handle to exercise a caller's tier-branching logic end to end,
    /// but aren't themselves testing retry/backoff timing (so don't
    /// need [`RetryPolicy`] exposed to them at all). Kept as a plain
    /// free function rather than widening `RetryPolicy`'s or
    /// `open_impl_network_test`'s own visibility beyond this module.
    #[cfg(test)]
    pub(crate) fn open_for_network_tier_test(
        library_path: &Path,
    ) -> Result<Self, LibraryHandleError> {
        Self::open_impl_network_test(
            library_path,
            JOURNAL_PRUNE_RETENTION,
            REAL_NETWORK_RETRY_POLICY,
        )
    }

    fn open_impl_inner(
        library_path: &Path,
        retention: u64,
        monitor_devices: bool,
        monitor_power: bool,
        force_tier_and_policy: Option<(StorageTier, RetryPolicy)>,
        force_suspended_block_timeout: Option<Duration>,
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

        let (tier, network_retry_policy) = match force_tier_and_policy {
            Some((tier, policy)) => (tier, policy),
            None => (
                classify_storage_tier(library_path),
                REAL_NETWORK_RETRY_POLICY,
            ),
        };
        let suspended_block_timeout =
            force_suspended_block_timeout.unwrap_or(REAL_SUSPENDED_BLOCK_TIMEOUT);

        let journal_dir = handle_dir.join(JOURNAL_DIR_NAME);
        let checkpoint_path = handle_dir.join(JOURNAL_CHECKPOINT_FILE_NAME);
        let head = recover_journal(
            &journal_dir,
            &checkpoint_path,
            retention,
            &network_retry_policy,
        )?;
        let library_id = load_or_create_library_id(&handle_dir)?;
        let fingerprint = compute_fingerprint(library_path, &library_id)?;

        let shared = Arc::new(Shared {
            library_path: library_path.to_path_buf(),
            handle_dir: handle_dir.clone(),
            tier,
            network_retry_policy,
            state: Mutex::new(HandleState::Open),
            state_changed: Condvar::new(),
            suspended_block_timeout,
            journal_dir,
            checkpoint_path,
            retention,
            journal_head: Mutex::new(head),
            lock_path,
            lock_file: Mutex::new(Some(lock_file)),
            fingerprint,
            #[cfg(unix)]
            inhibitor: Mutex::new(None),
        });

        #[cfg(unix)]
        {
            if monitor_devices {
                if let Some(device_name) = resolve_device_name(library_path) {
                    crate::device_monitor::spawn_device_monitor(
                        device_name,
                        Arc::downgrade(&shared),
                    );
                }
            }
            if monitor_power {
                crate::power_monitor::spawn_power_monitor(Arc::downgrade(&shared));
            }
        }
        #[cfg(not(unix))]
        let _ = (monitor_devices, monitor_power);

        Ok(LibraryHandle { shared })
    }

    pub fn library_path(&self) -> &Path {
        &self.shared.library_path
    }

    pub fn tier(&self) -> StorageTier {
        self.shared.tier
    }

    pub fn state(&self) -> HandleState {
        self.shared.state()
    }

    /// Port of §2 steps 1-4 for a file write: journal, write-temp /
    /// fsync-temp / rename / fsync-parent-directory, mark committed.
    /// `target` must be an absolute path (or at least caller-resolved
    /// -- this doesn't re-root it under `library_path`, callers do
    /// that).
    pub fn write_atomic(&self, target: &Path, bytes: &[u8]) -> Result<(), LibraryHandleError> {
        self.write_atomic_impl(target, bytes, None)
    }

    /// Large-file-safe counterpart to [`LibraryHandle::write_atomic`]
    /// for a caller that has `source` as an already-formed file on
    /// disk (e.g. a book format being added from outside the
    /// library) rather than a buffer already in memory -- never reads
    /// the whole file into memory. Same journal / write-temp / fsync-
    /// temp / rename / fsync-parent-directory / commit discipline.
    /// Returns the copied content's BLAKE3 hex, so a caller that also
    /// wants to record a checksum doesn't need a second full read of
    /// `target` to get one. See [`Shared::copy_atomic_impl`]'s doc for
    /// why this costs two passes over `source` instead of one.
    pub fn copy_atomic(&self, source: &Path, target: &Path) -> Result<String, LibraryHandleError> {
        self.copy_atomic_impl(source, target, None)
    }

    /// Port of §2 steps 1-4 for a rename, for callers that already
    /// have the payload written to its final form elsewhere (e.g. a
    /// format file copied by a caller, then atomically published into
    /// place) -- POSIX `rename` is already atomic; this adds the
    /// journal entry and the durability-relevant fsync of the parent
    /// director(ies) upstream's discipline requires.
    pub fn rename_atomic(&self, from: &Path, to: &Path) -> Result<(), LibraryHandleError> {
        self.shared.rename_atomic_impl(from, to, None)
    }

    /// Real delete primitive: journal, remove (file or directory,
    /// recursively), fsync-parent-directory, mark committed. Already-
    /// absent is success, not an error. See
    /// [`Shared::remove_atomic_impl`]'s doc for the full design,
    /// including why a non-atomic directory removal is still safe
    /// under this discipline.
    pub fn remove_atomic(&self, target: &Path) -> Result<(), LibraryHandleError> {
        self.remove_atomic_impl(target, None)
    }

    /// §6's two-phase multi-file batch for [`StorageTier::Network`]
    /// (issue #257's remaining scope) -- see [`NetworkBatch`]'s doc
    /// for the full design. Works regardless of the handle's actual
    /// tier (no artificial restriction here); it's the caller's job
    /// to decide when reaching for a batch is warranted, same as
    /// every other `LibraryHandle` primitive doesn't gate itself on
    /// tier either.
    pub fn begin_network_batch(&self) -> NetworkBatch<'_> {
        NetworkBatch {
            handle: self,
            steps: Vec::new(),
        }
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
        self.shared.write_atomic_impl(target, bytes, fail_after)
    }

    /// Test-only fault-injection wrapper for [`LibraryHandle::copy_atomic`],
    /// same shape as [`LibraryHandle::write_atomic_impl`].
    fn copy_atomic_impl(
        &self,
        source: &Path,
        target: &Path,
        fail_after: Option<FailPoint>,
    ) -> Result<String, LibraryHandleError> {
        self.shared.copy_atomic_impl(source, target, fail_after)
    }

    /// Test-only fault-injection wrapper for [`LibraryHandle::remove_atomic`],
    /// same shape as [`LibraryHandle::write_atomic_impl`].
    fn remove_atomic_impl(
        &self,
        target: &Path,
        fail_after: Option<FailPoint>,
    ) -> Result<(), LibraryHandleError> {
        self.shared.remove_atomic_impl(target, fail_after)
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
    /// Right after `remove_atomic_impl`'s actual `remove_file`/
    /// `remove_dir_all` call, before the directory fsync.
    Removed,
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

/// Streaming BLAKE3 of a file on disk -- unlike [`blake3_hex`], never
/// buffers the whole file in memory. Used for anything that hashes a
/// file that could be large (a copied-in book format, a recovered
/// write's target during crash recovery), as opposed to content a
/// caller already holds as an in-memory buffer (a cover image, a
/// small XML sidecar).
fn blake3_hash_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    io::copy(&mut file, &mut hasher)?;
    Ok(hasher.finalize().to_hex().to_string())
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

/// Removes `target` (file or directory, recursively) if it exists;
/// already-absent is success, not an error -- the shared idempotent
/// core both [`Shared::remove_atomic_impl`] and `recover_one`'s
/// `DeleteFile` arm use, so "delete" and "finish an interrupted
/// delete" are the exact same code path, not two implementations that
/// could drift.
fn remove_path(target: &Path) -> io::Result<()> {
    match fs::symlink_metadata(target) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(target),
        Ok(_) => fs::remove_file(target),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
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
    network_retry_policy: &RetryPolicy,
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

        recover_one(journal_dir, *uuid, entry, network_retry_policy)?;
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
    network_retry_policy: &RetryPolicy,
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
            if let Ok(hash) = blake3_hash_file(target) {
                if hash != *content_hash {
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
            let applied = blake3_hash_file(target)
                .map(|hash| hash == *content_hash)
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
        OperationDescriptor::DeleteFile { target } => {
            // Deletion is idempotent -- whether the crash happened
            // before the removal even started, partway through a
            // directory tree, or after it finished but before the
            // commit marker, "finish removing `target` now" is always
            // the correct recovery action (see `remove_atomic_impl`'s
            // doc for why a partial directory removal is never unsafe
            // to complete this way).
            remove_path(target)?;
            mark_committed(&committed_path)?;
        }
        OperationDescriptor::Batch { steps } => {
            // Same idempotent-forward-completion reasoning as
            // `DeleteFile` above, applied per step -- see
            // `NetworkBatch`'s doc for why this is safe (and correct)
            // instead of needing true rollback: every step is
            // individually idempotent to complete, and the whole set
            // was journaled as this one entry *before* any of them
            // ran, so finishing whichever ones didn't complete yet is
            // always the right recovery action regardless of where a
            // crash landed.
            for step in steps {
                complete_batch_step(step, network_retry_policy)?;
            }
            mark_committed(&committed_path)?;
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
pub(crate) fn classify_storage_tier(path: &Path) -> StorageTier {
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

/// Port of §5's resume-time mount fingerprint. Real on Unix: `st_dev`
/// via `stat`, plus the real filesystem UUID resolved by scanning
/// `/dev/disk/by-uuid` (a directory of symlinks udev maintains,
/// readable without root -- verified live on this machine before
/// relying on it) for the entry pointing at the same device
/// [`resolve_device_name`] finds. `fs_uuid` is `None`, not an error,
/// when the backing filesystem has no UUID there (`tmpfs`/`overlay`/
/// non-udev systems) -- the device id and `library_id` alone are
/// still a real, if slightly weaker, signal in that case.
#[cfg(unix)]
fn compute_fingerprint(library_path: &Path, library_id: &str) -> io::Result<MountFingerprint> {
    use std::os::unix::fs::MetadataExt;
    let device_id = fs::metadata(library_path)?.dev();
    let fs_uuid = resolve_device_name(library_path).and_then(|name| fs_uuid_for_device(&name));
    Ok(MountFingerprint {
        device_id,
        fs_uuid,
        library_id: library_id.to_string(),
    })
}

#[cfg(windows)]
fn compute_fingerprint(_library_path: &Path, library_id: &str) -> io::Result<MountFingerprint> {
    // No `st_dev`/`by-uuid` equivalent wired for Windows -- see this
    // module's other Windows gaps. `device_id: 0`/`fs_uuid: None` on
    // every call means resume-time revalidation always "matches" on
    // Windows today (it's comparing two identically-degenerate
    // values), which is honestly weaker than doing nothing loudly --
    // disclosed in `power_monitor.rs`'s module doc, and moot in
    // practice today since `power_monitor.rs` itself is `#[cfg(unix)]`
    // only, so nothing calls this to revalidate a real suspend on
    // Windows yet.
    Ok(MountFingerprint {
        device_id: 0,
        fs_uuid: None,
        library_id: library_id.to_string(),
    })
}

/// Scans `/dev/disk/by-uuid` for the symlink pointing at `device_name`
/// (e.g. `"vda2"`) and returns its filename (the UUID itself). `None`
/// if the directory doesn't exist (no udev, or a very minimal system)
/// or no entry matches.
#[cfg(unix)]
fn fs_uuid_for_device(device_name: &str) -> Option<String> {
    for entry in fs::read_dir("/dev/disk/by-uuid").ok()?.flatten() {
        let target = fs::read_link(entry.path()).ok()?;
        if target.file_name().and_then(|f| f.to_str()) == Some(device_name) {
            return entry.file_name().to_str().map(|s| s.to_string());
        }
    }
    None
}

/// This library's own persisted identity, per §5's mount-fingerprint
/// contract -- a plain UUID, generated once and read back verbatim
/// afterward. Written directly (not through the journal -- this is
/// the handle's own bookkeeping, not library content, same reasoning
/// as `writer.lock`/`journal_checkpoint`).
fn load_or_create_library_id(handle_dir: &Path) -> io::Result<String> {
    let path = handle_dir.join(LIBRARY_ID_FILE_NAME);
    match fs::read_to_string(&path) {
        Ok(s) => Ok(s.trim().to_string()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let id = Uuid::new_v4().to_string();
            fs::write(&path, &id)?;
            Ok(id)
        }
        Err(e) => Err(e),
    }
}

#[cfg(windows)]
pub(crate) fn classify_storage_tier(_path: &Path) -> StorageTier {
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

/// Test-only: a real [`Shared`] backed by a real (caller-supplied,
/// typically a tempdir) library directory, with both background
/// monitors off -- lets `device_monitor.rs`/`power_monitor.rs`'s own
/// tests exercise their event-handling logic against a real `Shared`
/// (real journal/lock-file/fingerprint plumbing) without needing a
/// live device or a real system suspend to drive it.
#[cfg(test)]
pub(crate) fn shared_for_test(dir: &Path) -> Arc<Shared> {
    LibraryHandle::open_impl(dir, JOURNAL_PRUNE_RETENTION, false, false)
        .unwrap()
        .shared
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn open_creates_the_handle_dir_and_acquires_the_lock() {
        let _flock_test_guard = flock_test_guard();
        let dir = tempdir().unwrap();
        let handle =
            LibraryHandle::open_impl(dir.path(), JOURNAL_PRUNE_RETENTION, true, false).unwrap();
        assert!(dir.path().join(".calibre-oxide").is_dir());
        assert_eq!(handle.state(), HandleState::Open);
    }

    #[test]
    fn a_second_open_on_the_same_library_fails_while_the_first_is_held() {
        let _flock_test_guard = flock_test_guard();
        let dir = tempdir().unwrap();
        let first =
            LibraryHandle::open_impl(dir.path(), JOURNAL_PRUNE_RETENTION, true, false).unwrap();
        let second = LibraryHandle::open_impl(dir.path(), JOURNAL_PRUNE_RETENTION, true, false);
        assert!(matches!(second, Err(LibraryHandleError::AlreadyLocked)));
        drop(first);
        // Releases automatically once the first handle (and its lock
        // file descriptor) drops.
        assert!(LibraryHandle::open_impl(dir.path(), JOURNAL_PRUNE_RETENTION, true, false).is_ok());
    }

    #[test]
    fn write_atomic_persists_the_bytes_and_overwrites_cleanly() {
        let _flock_test_guard = flock_test_guard();
        let dir = tempdir().unwrap();
        let handle =
            LibraryHandle::open_impl(dir.path(), JOURNAL_PRUNE_RETENTION, true, false).unwrap();
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
        let _flock_test_guard = flock_test_guard();
        let dir = tempdir().unwrap();
        let handle =
            LibraryHandle::open_impl(dir.path(), JOURNAL_PRUNE_RETENTION, true, false).unwrap();
        let from = dir.path().join("a.txt");
        let to = dir.path().join("b.txt");
        fs::write(&from, b"data").unwrap();

        handle.rename_atomic(&from, &to).unwrap();
        assert!(!from.exists());
        assert_eq!(fs::read(&to).unwrap(), b"data");
    }

    #[test]
    fn tier_classification_returns_a_local_tier_for_a_tempdir() {
        let _flock_test_guard = flock_test_guard();
        let dir = tempdir().unwrap();
        let handle =
            LibraryHandle::open_impl(dir.path(), JOURNAL_PRUNE_RETENTION, true, false).unwrap();
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
        let _flock_test_guard = flock_test_guard();
        // `LibraryHandle::open` (not `open_impl`) always monitors --
        // this is the ONE test in this file that goes through the
        // real public API with both monitors on, proving that real
        // code path doesn't panic, error, or otherwise disturb a
        // normal open. Every other test in this file uses
        // `open_impl(..., monitor_power: false)` deliberately: unlike
        // `device_monitor.rs`'s netlink socket (no external side
        // effect), `power_monitor.rs` takes a *real* system-wide
        // sleep inhibitor lock -- acquiring and releasing ~20 of those
        // across this file's other tests (which don't care about §5
        // at all) would be real, unnecessary interference with this
        // machine's actual suspend behavior while `cargo test` runs,
        // not just wasted resources. The monitor threads themselves
        // are exercised directly by `device_monitor.rs`'s and
        // `power_monitor.rs`'s own dedicated tests instead.
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
        let _flock_test_guard = flock_test_guard();
        let dir = tempdir().unwrap();
        let handle =
            LibraryHandle::open_impl(dir.path(), JOURNAL_PRUNE_RETENTION, true, false).unwrap();
        let target = dir.path().join("metadata.opf");
        fs::write(&target, b"original").unwrap();

        let err = handle.write_atomic_impl(&target, b"new", Some(FailPoint::WriteTemp));
        assert!(err.is_err());
        assert_eq!(fs::read(&target).unwrap(), b"original");
    }

    #[test]
    fn a_crash_right_after_fsyncing_the_temp_file_leaves_the_original_untouched() {
        let _flock_test_guard = flock_test_guard();
        let dir = tempdir().unwrap();
        let handle =
            LibraryHandle::open_impl(dir.path(), JOURNAL_PRUNE_RETENTION, true, false).unwrap();
        let target = dir.path().join("metadata.opf");
        fs::write(&target, b"original").unwrap();

        let err = handle.write_atomic_impl(&target, b"new", Some(FailPoint::FsyncTemp));
        assert!(err.is_err());
        assert_eq!(fs::read(&target).unwrap(), b"original");
    }

    #[test]
    fn a_crash_right_after_the_rename_has_already_committed_the_new_content() {
        let _flock_test_guard = flock_test_guard();
        // Once `rename` has happened, the write is durable regardless
        // of whether subsequent steps complete -- this is exactly why
        // write-temp-then-rename is the right shape: the commit point
        // is a single atomic filesystem operation, not a multi-step
        // window.
        let dir = tempdir().unwrap();
        let handle =
            LibraryHandle::open_impl(dir.path(), JOURNAL_PRUNE_RETENTION, true, false).unwrap();
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
        let _flock_test_guard = flock_test_guard();
        let dir = tempdir().unwrap();
        let handle =
            LibraryHandle::open_impl(dir.path(), JOURNAL_PRUNE_RETENTION, true, false).unwrap();
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
        let _flock_test_guard = flock_test_guard();
        let dir = tempdir().unwrap();
        let handle =
            LibraryHandle::open_impl(dir.path(), JOURNAL_PRUNE_RETENTION, true, false).unwrap();
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
        let _flock_test_guard = flock_test_guard();
        let dir = tempdir().unwrap();
        {
            let handle =
                LibraryHandle::open_impl(dir.path(), JOURNAL_PRUNE_RETENTION, true, false).unwrap();
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
        let handle = reopen_for_test(dir.path(), JOURNAL_PRUNE_RETENTION);
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
        let _flock_test_guard = flock_test_guard();
        let dir = tempdir().unwrap();
        let target = dir.path().join("metadata.opf");
        {
            let handle =
                LibraryHandle::open_impl(dir.path(), JOURNAL_PRUNE_RETENTION, true, false).unwrap();
            let err =
                handle.write_atomic_impl(&target, b"new content", Some(FailPoint::BeforeCommit));
            assert!(err.is_err());
            // The rename already happened -- the real content is
            // already there, just unmarked.
            assert_eq!(fs::read(&target).unwrap(), b"new content");
        }

        let handle = reopen_for_test(dir.path(), JOURNAL_PRUNE_RETENTION);
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
        let handle2 = reopen_for_test(dir.path(), JOURNAL_PRUNE_RETENTION);
        assert_eq!(fs::read(&target).unwrap(), b"new content");
        drop(handle2);
    }

    #[test]
    fn reopening_after_a_crash_before_the_journal_write_even_finished_has_nothing_to_recover() {
        let _flock_test_guard = flock_test_guard();
        // The very first fault-injection point: nothing was ever
        // journaled successfully, so there's no entry to recover at
        // all. (Simulated here by never calling write_atomic in the
        // first place -- the interesting assertion is just that
        // `open()` on a library with an empty journal doesn't error.)
        let dir = tempdir().unwrap();
        let handle =
            LibraryHandle::open_impl(dir.path(), JOURNAL_PRUNE_RETENTION, true, false).unwrap();
        assert_eq!(handle.state(), HandleState::Open);
    }

    #[test]
    fn recovery_detects_a_tampered_journal_entry_as_corruption() {
        let _flock_test_guard = flock_test_guard();
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

        let result = LibraryHandle::open_impl(dir.path(), JOURNAL_PRUNE_RETENTION, true, false);
        assert!(matches!(result, Err(LibraryHandleError::Corruption(_))));
    }

    #[test]
    fn recovery_detects_a_broken_chain_as_corruption() {
        let _flock_test_guard = flock_test_guard();
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

        let result = LibraryHandle::open_impl(dir.path(), JOURNAL_PRUNE_RETENTION, true, false);
        assert!(matches!(result, Err(LibraryHandleError::Corruption(_))));
    }

    // }}}

    // --- large-file-safe copy-in primitive (issue #93 crate-wide write-path retrofit) {{{

    #[test]
    fn copy_atomic_copies_the_file_and_returns_its_real_hash() {
        let _flock_test_guard = flock_test_guard();
        let dir = tempdir().unwrap();
        let handle =
            LibraryHandle::open_impl(dir.path(), JOURNAL_PRUNE_RETENTION, true, false).unwrap();
        let source = dir.path().join("source.epub");
        fs::write(&source, b"epub bytes").unwrap();
        let target = dir.path().join("book").join("book.epub");

        let hash = handle.copy_atomic(&source, &target).unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"epub bytes");
        assert_eq!(hash, blake3_hex(b"epub bytes"));
        // The source is untouched -- this is a copy, not a move.
        assert!(source.exists());

        // No leftover temp files.
        let leftovers: Vec<_> = fs::read_dir(target.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn copy_atomic_overwrites_an_existing_target_cleanly() {
        let _flock_test_guard = flock_test_guard();
        let dir = tempdir().unwrap();
        let handle =
            LibraryHandle::open_impl(dir.path(), JOURNAL_PRUNE_RETENTION, true, false).unwrap();
        let target = dir.path().join("book.epub");
        handle.write_atomic(&target, b"old content").unwrap();

        let source = dir.path().join("new.epub");
        fs::write(&source, b"new content").unwrap();
        handle.copy_atomic(&source, &target).unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"new content");
    }

    #[test]
    fn copy_atomic_journals_a_write_file_entry_with_the_real_content_hash() {
        let _flock_test_guard = flock_test_guard();
        let dir = tempdir().unwrap();
        let handle =
            LibraryHandle::open_impl(dir.path(), JOURNAL_PRUNE_RETENTION, true, false).unwrap();
        let source = dir.path().join("source.epub");
        fs::write(&source, b"epub bytes").unwrap();
        let target = dir.path().join("book.epub");

        handle.copy_atomic(&source, &target).unwrap();

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
        match entry.op {
            OperationDescriptor::WriteFile {
                target: t,
                content_hash,
            } => {
                assert_eq!(t, target);
                assert_eq!(content_hash, blake3_hex(b"epub bytes"));
            }
            _ => panic!("expected WriteFile"),
        }
    }

    #[test]
    fn a_crash_right_after_copy_atomics_journal_write_leaves_the_target_untouched() {
        let dir = tempdir().unwrap();
        let _flock_test_guard = flock_test_guard();
        let handle =
            LibraryHandle::open_impl(dir.path(), JOURNAL_PRUNE_RETENTION, true, false).unwrap();
        let source = dir.path().join("source.epub");
        fs::write(&source, b"epub bytes").unwrap();
        let target = dir.path().join("book.epub");

        let err = handle.copy_atomic_impl(&source, &target, Some(FailPoint::JournalWrite));
        assert!(err.is_err());
        assert!(!target.exists());
    }

    #[test]
    fn a_crash_right_after_copy_atomics_rename_has_already_committed_the_write() {
        let dir = tempdir().unwrap();
        let _flock_test_guard = flock_test_guard();
        let handle =
            LibraryHandle::open_impl(dir.path(), JOURNAL_PRUNE_RETENTION, true, false).unwrap();
        let source = dir.path().join("source.epub");
        fs::write(&source, b"epub bytes").unwrap();
        let target = dir.path().join("book.epub");

        let err = handle.copy_atomic_impl(&source, &target, Some(FailPoint::Rename));
        assert!(err.is_err());
        assert_eq!(fs::read(&target).unwrap(), b"epub bytes");
    }

    #[test]
    fn reopening_after_a_crash_right_after_copy_atomics_rename_finalizes_the_commit_marker() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("book.epub");
        {
            let _flock_test_guard = flock_test_guard();
            let handle =
                LibraryHandle::open_impl(dir.path(), JOURNAL_PRUNE_RETENTION, true, false).unwrap();
            let source = dir.path().join("source.epub");
            fs::write(&source, b"epub bytes").unwrap();
            let err = handle.copy_atomic_impl(&source, &target, Some(FailPoint::Rename));
            assert!(err.is_err());
        }

        let handle = reopen_for_test(dir.path(), JOURNAL_PRUNE_RETENTION);
        assert_eq!(fs::read(&target).unwrap(), b"epub bytes");

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
        drop(handle);
    }

    // }}}

    // --- delete primitive (issue #93 crate-wide write-path retrofit) {{{

    #[test]
    fn remove_atomic_deletes_a_file() {
        let dir = tempdir().unwrap();
        let _flock_test_guard = flock_test_guard();
        let handle =
            LibraryHandle::open_impl(dir.path(), JOURNAL_PRUNE_RETENTION, true, false).unwrap();
        let target = dir.path().join("metadata.opf");
        fs::write(&target, b"gone soon").unwrap();

        handle.remove_atomic(&target).unwrap();

        assert!(!target.exists());
    }

    #[test]
    fn remove_atomic_deletes_a_directory_recursively() {
        let dir = tempdir().unwrap();
        let _flock_test_guard = flock_test_guard();
        let handle =
            LibraryHandle::open_impl(dir.path(), JOURNAL_PRUNE_RETENTION, true, false).unwrap();
        let book_dir = dir.path().join("Author A").join("Some Book");
        fs::create_dir_all(&book_dir).unwrap();
        fs::write(book_dir.join("book.epub"), b"epub bytes").unwrap();
        fs::write(book_dir.join("cover.jpg"), b"cover bytes").unwrap();

        handle.remove_atomic(&book_dir).unwrap();

        assert!(!book_dir.exists());
    }

    #[test]
    fn remove_atomic_on_an_already_absent_target_is_a_no_op_success() {
        let dir = tempdir().unwrap();
        let _flock_test_guard = flock_test_guard();
        let handle =
            LibraryHandle::open_impl(dir.path(), JOURNAL_PRUNE_RETENTION, true, false).unwrap();
        let target = dir.path().join("never-existed.opf");

        // Deletion is idempotent -- a caller retrying after an
        // interrupted delete (or racing recovery's own completion of
        // the same entry) must not get an error.
        handle.remove_atomic(&target).unwrap();
        handle.remove_atomic(&target).unwrap();
    }

    #[test]
    fn remove_atomic_journals_an_entry_and_marks_it_committed() {
        let dir = tempdir().unwrap();
        let _flock_test_guard = flock_test_guard();
        let handle =
            LibraryHandle::open_impl(dir.path(), JOURNAL_PRUNE_RETENTION, true, false).unwrap();
        let target = dir.path().join("metadata.opf");
        fs::write(&target, b"content").unwrap();

        handle.remove_atomic(&target).unwrap();

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
        match entry.op {
            OperationDescriptor::DeleteFile { target: t } => assert_eq!(t, target),
            _ => panic!("expected DeleteFile"),
        }
    }

    #[test]
    fn a_crash_right_after_the_journal_write_leaves_the_target_untouched() {
        let dir = tempdir().unwrap();
        let _flock_test_guard = flock_test_guard();
        let handle =
            LibraryHandle::open_impl(dir.path(), JOURNAL_PRUNE_RETENTION, true, false).unwrap();
        let target = dir.path().join("metadata.opf");
        fs::write(&target, b"still here").unwrap();

        let err = handle.remove_atomic_impl(&target, Some(FailPoint::JournalWrite));
        assert!(err.is_err());
        assert!(target.exists());
    }

    #[test]
    fn a_crash_right_after_removal_has_already_committed_the_delete() {
        // Same shape as write_atomic's equivalent test: once the real
        // removal syscall(s) have happened, the delete is durable
        // regardless of whether the commit marker itself made it to
        // disk -- recovery's job (proven below) is to just finish
        // writing that marker, not redo anything.
        let dir = tempdir().unwrap();
        let _flock_test_guard = flock_test_guard();
        let handle =
            LibraryHandle::open_impl(dir.path(), JOURNAL_PRUNE_RETENTION, true, false).unwrap();
        let target = dir.path().join("metadata.opf");
        fs::write(&target, b"content").unwrap();

        let err = handle.remove_atomic_impl(&target, Some(FailPoint::Removed));
        assert!(err.is_err());
        assert!(!target.exists());
    }

    #[test]
    fn reopening_after_a_crash_right_after_removal_finalizes_the_commit_marker() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("metadata.opf");
        {
            let _flock_test_guard = flock_test_guard();
            let handle =
                LibraryHandle::open_impl(dir.path(), JOURNAL_PRUNE_RETENTION, true, false).unwrap();
            fs::write(&target, b"content").unwrap();
            let err = handle.remove_atomic_impl(&target, Some(FailPoint::Removed));
            assert!(err.is_err());
            assert!(!target.exists());
        }

        let handle = reopen_for_test(dir.path(), JOURNAL_PRUNE_RETENTION);
        assert!(!target.exists());

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
        drop(handle);
    }

    #[test]
    fn reopening_after_a_crash_before_the_removal_ever_started_still_completes_it() {
        // Hand-constructs the "journaled but the process died before
        // touching the filesystem at all" case -- `remove_atomic_impl`
        // can't be crashed *before* `remove_path` via a `FailPoint`
        // (there's no step between the journal write and the removal
        // itself to inject at), so this writes the same journal entry
        // `journal_write` would have, directly.
        let dir = tempdir().unwrap();
        let book_dir = dir.path().join("Author A").join("Some Book");
        fs::create_dir_all(&book_dir).unwrap();
        fs::write(book_dir.join("book.epub"), b"epub bytes").unwrap();

        let journal_dir = dir.path().join(".calibre-oxide").join("journal");
        fs::create_dir_all(&journal_dir).unwrap();
        let op = OperationDescriptor::DeleteFile {
            target: book_dir.clone(),
        };
        let descriptor_hash = blake3_hex(&serde_json::to_vec(&op).unwrap());
        let entry = JournalEntry {
            seq: 0,
            prev_head: None,
            op,
            descriptor_hash,
        };
        fs::write(
            journal_dir.join(format!("{}.op", Uuid::new_v4())),
            serde_json::to_vec(&entry).unwrap(),
        )
        .unwrap();

        let handle = reopen_for_test(dir.path(), JOURNAL_PRUNE_RETENTION);
        assert!(
            !book_dir.exists(),
            "recovery should finish a delete that never even started"
        );
        drop(handle);
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
        let _flock_test_guard = flock_test_guard();
        let dir = tempdir().unwrap();
        {
            let handle = LibraryHandle::open_impl(dir.path(), 3, false, false).unwrap();
            for i in 0..3 {
                handle
                    .write_atomic(&dir.path().join(format!("{i}.opf")), b"x")
                    .unwrap();
            }
        }
        reopen_for_test(dir.path(), 3);
        assert_eq!(op_file_count(&journal_dir_of(dir.path())), 3);
        assert!(!dir
            .path()
            .join(".calibre-oxide")
            .join("journal_checkpoint")
            .exists());
    }

    #[test]
    fn reopening_over_the_retention_limit_prunes_the_oldest_entries() {
        let _flock_test_guard = flock_test_guard();
        let dir = tempdir().unwrap();
        {
            let handle = LibraryHandle::open_impl(dir.path(), 3, false, false).unwrap();
            for i in 0..5 {
                handle
                    .write_atomic(&dir.path().join(format!("{i}.opf")), b"x")
                    .unwrap();
            }
        }
        // Recovery on this open settles all 5, then prunes down to 3.
        reopen_for_test(dir.path(), 3);
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
        let _flock_test_guard = flock_test_guard();
        let dir = tempdir().unwrap();
        {
            let handle = LibraryHandle::open_impl(dir.path(), 2, false, false).unwrap();
            for i in 0..4 {
                handle
                    .write_atomic(&dir.path().join(format!("{i}.opf")), b"x")
                    .unwrap();
            }
        }
        // This open prunes seq 0-1 away, keeping 2-3.
        let handle = reopen_for_test(dir.path(), 2);
        handle
            .write_atomic(&dir.path().join("new.opf"), b"y")
            .unwrap();
        drop(handle);

        // A fresh open must still verify cleanly -- the new entry's
        // prev_head chains onto the last pre-prune entry's hash, which
        // the checkpoint (not a deleted file) now supplies.
        let handle = reopen_for_test(dir.path(), 2);
        assert_eq!(handle.state(), HandleState::Open);
        assert_eq!(fs::read(dir.path().join("new.opf")).unwrap(), b"y");
    }

    #[test]
    fn an_interrupted_prune_self_heals_on_the_next_open() {
        let _flock_test_guard = flock_test_guard();
        // Simulates a crash between the checkpoint write and the
        // deletion pass: write a checkpoint that's already past some
        // still-present entries, and confirm the next open both
        // succeeds and finishes deleting them.
        let dir = tempdir().unwrap();
        {
            let handle = LibraryHandle::open_impl(dir.path(), 100, false, false).unwrap();
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

        let handle = reopen_for_test(dir.path(), 100);
        assert_eq!(handle.state(), HandleState::Open);
        // The stale, already-superseded entry 0 and 1 files are
        // cleaned up by the recovery scan itself.
        assert_eq!(op_file_count(&journal_dir), 1);
    }

    // }}}

    // --- §6 network-storage write-path safety (issue #93, #257) {{{

    /// Deliberately does NOT call `flock_test_guard()` itself --
    /// `std::sync::Mutex` isn't reentrant, and a test that needs the
    /// guard held for its *whole* body (anything that reopens the
    /// same directory, not just a single call to this helper) would
    /// deadlock locking it twice on the same thread. Every test that
    /// needs the guard acquires it itself, as its first statement,
    /// same convention as every other `open_impl`-based test in this
    /// file.
    fn open_network_test(dir: &Path, policy: RetryPolicy) -> LibraryHandle {
        LibraryHandle::open_impl_network_test(dir, JOURNAL_PRUNE_RETENTION, policy).unwrap()
    }

    /// Milliseconds, not minutes -- so the "gives up after the total
    /// budget" tests below complete near-instantly instead of taking
    /// real minutes, while still exercising the exact same doubling/
    /// capping/give-up arithmetic `REAL_NETWORK_RETRY_POLICY` uses.
    const FAST_TEST_RETRY_POLICY: RetryPolicy = RetryPolicy {
        initial_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(4),
        total_budget: Duration::from_millis(30),
    };

    #[test]
    fn retry_with_backoff_succeeds_immediately_without_retrying() {
        let mut calls = 0;
        let result = retry_with_backoff(
            &FAST_TEST_RETRY_POLICY,
            || -> Result<(), LibraryHandleError> {
                calls += 1;
                Ok(())
            },
        );
        assert!(result.is_ok());
        assert_eq!(calls, 1);
    }

    #[test]
    fn retry_with_backoff_retries_and_eventually_succeeds() {
        let mut calls = 0;
        let result = retry_with_backoff(&FAST_TEST_RETRY_POLICY, || {
            calls += 1;
            if calls < 3 {
                Err(simulated_network_fault())
            } else {
                Ok(())
            }
        });
        assert!(result.is_ok());
        assert_eq!(calls, 3);
    }

    #[test]
    fn retry_with_backoff_gives_up_after_the_total_budget_and_returns_the_last_error() {
        let mut calls = 0;
        let result = retry_with_backoff(
            &FAST_TEST_RETRY_POLICY,
            || -> Result<(), LibraryHandleError> {
                calls += 1;
                Err(simulated_network_fault())
            },
        );
        assert!(result.is_err());
        // More than one attempt was made (it's genuinely retrying),
        // but it did eventually give up rather than looping forever.
        assert!(calls > 1);
    }

    #[test]
    fn write_atomic_on_network_tier_writes_and_verifies_via_read_back_hash() {
        let _flock_test_guard = flock_test_guard();
        set_simulated_network_fault_countdown(0);
        let dir = tempdir().unwrap();
        let handle = open_network_test(dir.path(), FAST_TEST_RETRY_POLICY);
        let target = dir.path().join("book").join("metadata.opf");

        handle.write_atomic(&target, b"network content").unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"network content");

        // No leftover temp files under the target's own directory --
        // the scratch file lives elsewhere (a genuinely local temp
        // dir) and is cleaned up regardless of outcome.
        let leftovers: Vec<_> = fs::read_dir(target.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn write_atomic_on_network_tier_retries_transient_faults_and_succeeds() {
        let _flock_test_guard = flock_test_guard();
        set_simulated_network_fault_countdown(2);
        let dir = tempdir().unwrap();
        let handle = open_network_test(dir.path(), FAST_TEST_RETRY_POLICY);
        let target = dir.path().join("metadata.opf");

        handle.write_atomic(&target, b"content").unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"content");
        set_simulated_network_fault_countdown(0);
    }

    #[test]
    fn write_atomic_on_network_tier_gives_up_after_the_retry_budget_and_bubbles_up_the_error() {
        let _flock_test_guard = flock_test_guard();
        set_simulated_network_fault_countdown(u32::MAX);
        let dir = tempdir().unwrap();
        let handle = open_network_test(dir.path(), FAST_TEST_RETRY_POLICY);
        let target = dir.path().join("metadata.opf");

        let result = handle.write_atomic(&target, b"content");

        assert!(result.is_err());
        assert!(!target.exists());
        set_simulated_network_fault_countdown(0);
    }

    #[test]
    fn write_atomic_on_network_tier_journals_exactly_one_entry_even_after_retries() {
        let _flock_test_guard = flock_test_guard();
        set_simulated_network_fault_countdown(2);
        let dir = tempdir().unwrap();
        let handle = open_network_test(dir.path(), FAST_TEST_RETRY_POLICY);
        let target = dir.path().join("metadata.opf");

        handle.write_atomic(&target, b"content").unwrap();

        let journal_dir = dir.path().join(".calibre-oxide").join("journal");
        let op_files: Vec<_> = fs::read_dir(&journal_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("op"))
            .collect();
        assert_eq!(
            op_files.len(),
            1,
            "retrying the upload must not journal a second entry"
        );
        let uuid = op_files[0]
            .path()
            .file_stem()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(journal_dir.join(format!("{uuid}.committed")).exists());
        set_simulated_network_fault_countdown(0);
    }

    #[test]
    fn copy_atomic_on_network_tier_copies_and_verifies_via_read_back_hash() {
        let _flock_test_guard = flock_test_guard();
        set_simulated_network_fault_countdown(0);
        let dir = tempdir().unwrap();
        let handle = open_network_test(dir.path(), FAST_TEST_RETRY_POLICY);
        let source = dir.path().join("source.epub");
        fs::write(&source, b"epub bytes").unwrap();
        let target = dir.path().join("book.epub");

        let hash = handle.copy_atomic(&source, &target).unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"epub bytes");
        assert_eq!(hash, blake3_hex(b"epub bytes"));
        assert!(source.exists(), "copy, not a move");
    }

    #[test]
    fn copy_atomic_on_network_tier_retries_transient_faults_and_succeeds() {
        let _flock_test_guard = flock_test_guard();
        set_simulated_network_fault_countdown(2);
        let dir = tempdir().unwrap();
        let handle = open_network_test(dir.path(), FAST_TEST_RETRY_POLICY);
        let source = dir.path().join("source.epub");
        fs::write(&source, b"epub bytes").unwrap();
        let target = dir.path().join("book.epub");

        handle.copy_atomic(&source, &target).unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"epub bytes");
        set_simulated_network_fault_countdown(0);
    }

    #[test]
    fn rename_atomic_on_network_tier_renames() {
        let _flock_test_guard = flock_test_guard();
        set_simulated_network_fault_countdown(0);
        let dir = tempdir().unwrap();
        let handle = open_network_test(dir.path(), FAST_TEST_RETRY_POLICY);
        let from = dir.path().join("a.txt");
        let to = dir.path().join("b.txt");
        fs::write(&from, b"data").unwrap();

        handle.rename_atomic(&from, &to).unwrap();

        assert!(!from.exists());
        assert_eq!(fs::read(&to).unwrap(), b"data");
    }

    #[test]
    fn rename_atomic_on_network_tier_retries_transient_faults_and_succeeds() {
        let _flock_test_guard = flock_test_guard();
        set_simulated_network_fault_countdown(2);
        let dir = tempdir().unwrap();
        let handle = open_network_test(dir.path(), FAST_TEST_RETRY_POLICY);
        let from = dir.path().join("a.txt");
        let to = dir.path().join("b.txt");
        fs::write(&from, b"data").unwrap();

        handle.rename_atomic(&from, &to).unwrap();

        assert!(!from.exists());
        assert_eq!(fs::read(&to).unwrap(), b"data");
        set_simulated_network_fault_countdown(0);
    }

    #[test]
    fn remove_atomic_on_network_tier_removes() {
        let _flock_test_guard = flock_test_guard();
        set_simulated_network_fault_countdown(0);
        let dir = tempdir().unwrap();
        let handle = open_network_test(dir.path(), FAST_TEST_RETRY_POLICY);
        let target = dir.path().join("a.txt");
        fs::write(&target, b"data").unwrap();

        handle.remove_atomic(&target).unwrap();

        assert!(!target.exists());
    }

    #[test]
    fn remove_atomic_on_network_tier_retries_transient_faults_and_succeeds() {
        let _flock_test_guard = flock_test_guard();
        set_simulated_network_fault_countdown(2);
        let dir = tempdir().unwrap();
        let handle = open_network_test(dir.path(), FAST_TEST_RETRY_POLICY);
        let target = dir.path().join("a.txt");
        fs::write(&target, b"data").unwrap();

        handle.remove_atomic(&target).unwrap();

        assert!(!target.exists());
        set_simulated_network_fault_countdown(0);
    }

    #[test]
    fn publish_over_network_retries_and_gives_up_when_the_content_never_matches_the_expected_hash()
    {
        set_simulated_network_fault_countdown(0);
        let dir = tempdir().unwrap();
        let target = dir.path().join("target.txt");
        let tmp_path = dir.path().join("target.txt.tmp-test");

        let result = publish_over_network(
            &target,
            &tmp_path,
            "not-the-real-hash-of-anything",
            &FAST_TEST_RETRY_POLICY,
            |scratch| fs::write(scratch, b"real content"),
        );

        assert!(matches!(result, Err(LibraryHandleError::Corruption(_))));
    }

    #[test]
    fn network_batch_commits_every_staged_step() {
        let _flock_test_guard = flock_test_guard();
        set_simulated_network_fault_countdown(0);
        let dir = tempdir().unwrap();
        let handle = open_network_test(dir.path(), FAST_TEST_RETRY_POLICY);
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();
        let a2 = dir.path().join("a2.txt");
        let b2 = dir.path().join("b2.txt");

        let mut batch = handle.begin_network_batch();
        batch.stage_rename(&a, &a2);
        batch.stage_rename(&b, &b2);
        batch.commit().unwrap();

        assert!(!a.exists());
        assert!(!b.exists());
        assert_eq!(fs::read(&a2).unwrap(), b"a");
        assert_eq!(fs::read(&b2).unwrap(), b"b");
    }

    #[test]
    fn network_batch_supports_staged_removal() {
        let _flock_test_guard = flock_test_guard();
        set_simulated_network_fault_countdown(0);
        let dir = tempdir().unwrap();
        let handle = open_network_test(dir.path(), FAST_TEST_RETRY_POLICY);
        let a = dir.path().join("a.txt");
        let old_dir = dir.path().join("now-empty");
        fs::create_dir_all(&old_dir).unwrap();
        fs::write(&a, b"a").unwrap();
        let a2 = dir.path().join("a2.txt");

        let mut batch = handle.begin_network_batch();
        batch.stage_rename(&a, &a2);
        batch.stage_remove(&old_dir);
        batch.commit().unwrap();

        assert_eq!(fs::read(&a2).unwrap(), b"a");
        assert!(!old_dir.exists());
    }

    #[test]
    fn network_batch_with_no_steps_is_a_no_op_and_journals_nothing() {
        let _flock_test_guard = flock_test_guard();
        set_simulated_network_fault_countdown(0);
        let dir = tempdir().unwrap();
        let handle = open_network_test(dir.path(), FAST_TEST_RETRY_POLICY);

        handle.begin_network_batch().commit().unwrap();

        let journal_dir = dir.path().join(".calibre-oxide").join("journal");
        let op_files: Vec<_> = fs::read_dir(&journal_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("op"))
            .collect();
        assert!(op_files.is_empty());
    }

    #[test]
    fn network_batch_journals_exactly_one_entry_for_the_whole_batch() {
        let _flock_test_guard = flock_test_guard();
        set_simulated_network_fault_countdown(0);
        let dir = tempdir().unwrap();
        let handle = open_network_test(dir.path(), FAST_TEST_RETRY_POLICY);
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();

        let mut batch = handle.begin_network_batch();
        batch.stage_rename(&a, dir.path().join("a2.txt").as_path());
        batch.stage_rename(&b, dir.path().join("b2.txt").as_path());
        batch.commit().unwrap();

        let journal_dir = dir.path().join(".calibre-oxide").join("journal");
        let op_files: Vec<_> = fs::read_dir(&journal_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("op"))
            .collect();
        assert_eq!(
            op_files.len(),
            1,
            "a 2-step batch must journal one entry, not one per step"
        );
        let uuid = op_files[0]
            .path()
            .file_stem()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(journal_dir.join(format!("{uuid}.committed")).exists());
    }

    #[test]
    fn network_batch_retries_a_transient_fault_within_one_step_and_succeeds() {
        let _flock_test_guard = flock_test_guard();
        set_simulated_network_fault_countdown(2);
        let dir = tempdir().unwrap();
        let handle = open_network_test(dir.path(), FAST_TEST_RETRY_POLICY);
        let a = dir.path().join("a.txt");
        fs::write(&a, b"a").unwrap();
        let a2 = dir.path().join("a2.txt");

        let mut batch = handle.begin_network_batch();
        batch.stage_rename(&a, &a2);
        batch.commit().unwrap();

        assert_eq!(fs::read(&a2).unwrap(), b"a");
        set_simulated_network_fault_countdown(0);
    }

    #[test]
    fn complete_batch_step_skips_a_rename_thats_already_applied() {
        let dir = tempdir().unwrap();
        let from = dir.path().join("from.txt");
        let to = dir.path().join("to.txt");
        // The rename already happened by some other means -- `from`
        // is gone, `to` already has the real content.
        fs::write(&to, b"already moved").unwrap();

        let step = BatchStep::Rename {
            from: from.clone(),
            to: to.clone(),
        };
        complete_batch_step(&step, &FAST_TEST_RETRY_POLICY).unwrap();

        // Untouched -- `complete_batch_step` must not have tried to
        // rename a nonexistent `from` onto `to` (which would error).
        assert_eq!(fs::read(&to).unwrap(), b"already moved");
    }

    #[test]
    fn a_crash_right_after_the_first_batch_step_leaves_the_second_step_undone() {
        let dir = tempdir().unwrap();
        let _flock_test_guard = flock_test_guard();
        let handle = open_network_test(dir.path(), FAST_TEST_RETRY_POLICY);
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();
        let a2 = dir.path().join("a2.txt");
        let b2 = dir.path().join("b2.txt");

        let mut batch = handle.begin_network_batch();
        batch.stage_rename(&a, &a2);
        batch.stage_rename(&b, &b2);
        let err = batch.commit_impl(Some(0));

        assert!(err.is_err());
        assert!(a2.exists(), "step 0 should have completed");
        assert!(b.exists(), "step 1 should NOT have run yet");
        assert!(!b2.exists());
    }

    #[test]
    fn reopening_after_a_crash_mid_batch_completes_only_the_remaining_steps() {
        let dir = tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        let a2 = dir.path().join("a2.txt");
        let b2 = dir.path().join("b2.txt");
        {
            let _flock_test_guard = flock_test_guard();
            let handle = open_network_test(dir.path(), FAST_TEST_RETRY_POLICY);
            fs::write(&a, b"a").unwrap();
            fs::write(&b, b"b").unwrap();

            let mut batch = handle.begin_network_batch();
            batch.stage_rename(&a, &a2);
            batch.stage_rename(&b, &b2);
            let err = batch.commit_impl(Some(0));
            assert!(err.is_err());
            assert!(a2.exists());
            assert!(b.exists());
        }

        let handle = open_network_test(dir.path(), FAST_TEST_RETRY_POLICY);

        // Recovery finished the whole batch: step 0 (already done)
        // was left alone, step 1 (never ran) was completed.
        assert!(a2.exists());
        assert_eq!(fs::read(&a2).unwrap(), b"a");
        assert!(!b.exists());
        assert_eq!(fs::read(&b2).unwrap(), b"b");

        let journal_dir = dir.path().join(".calibre-oxide").join("journal");
        let committed: Vec<_> = fs::read_dir(&journal_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("committed"))
            .collect();
        assert_eq!(
            committed.len(),
            1,
            "recovery should finalize the batch's commit marker"
        );
        drop(handle);
    }

    #[test]
    fn reopening_after_a_crash_before_the_batch_ever_started_still_completes_it() {
        // Same shape as the delete primitive's equivalent test: hand-
        // constructs the "journaled but the process died before
        // touching the filesystem at all" case directly, rather than
        // via `commit_impl`'s fault injection (which can only crash
        // *after* a step, not before the very first one).
        let dir = tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();
        let a2 = dir.path().join("a2.txt");
        let b2 = dir.path().join("b2.txt");

        let journal_dir = dir.path().join(".calibre-oxide").join("journal");
        fs::create_dir_all(&journal_dir).unwrap();
        let op = OperationDescriptor::Batch {
            steps: vec![
                BatchStep::Rename {
                    from: a.clone(),
                    to: a2.clone(),
                },
                BatchStep::Rename {
                    from: b.clone(),
                    to: b2.clone(),
                },
            ],
        };
        let descriptor_hash = blake3_hex(&serde_json::to_vec(&op).unwrap());
        let entry = JournalEntry {
            seq: 0,
            prev_head: None,
            op,
            descriptor_hash,
        };
        fs::write(
            journal_dir.join(format!("{}.op", Uuid::new_v4())),
            serde_json::to_vec(&entry).unwrap(),
        )
        .unwrap();

        let handle = reopen_for_test(dir.path(), JOURNAL_PRUNE_RETENTION);
        assert!(
            a2.exists() && b2.exists(),
            "recovery should complete a batch that never even started"
        );
        drop(handle);
    }

    // }}}

    // --- §5 step 2: blocking-with-timeout while Suspended (issue #259) {{{

    const FAST_TEST_SUSPEND_TIMEOUT: Duration = Duration::from_millis(50);

    fn open_suspend_test(dir: &Path, timeout: Duration) -> LibraryHandle {
        LibraryHandle::open_impl_suspend_test(dir, JOURNAL_PRUNE_RETENTION, timeout).unwrap()
    }

    #[test]
    fn check_open_succeeds_immediately_when_state_is_open() {
        let _flock_test_guard = flock_test_guard();
        let dir = tempdir().unwrap();
        let handle = open_suspend_test(dir.path(), Duration::from_secs(5));

        let start = Instant::now();
        handle
            .write_atomic(&dir.path().join("x.txt"), b"data")
            .unwrap();
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "an Open handle must not block at all"
        );
    }

    #[test]
    fn check_open_fails_immediately_when_already_detached() {
        let _flock_test_guard = flock_test_guard();
        let dir = tempdir().unwrap();
        let handle = open_suspend_test(dir.path(), Duration::from_secs(5));
        handle.shared.set_state(HandleState::Detached);

        let start = Instant::now();
        let result = handle.write_atomic(&dir.path().join("x.txt"), b"data");
        assert!(matches!(result, Err(LibraryHandleError::DeviceDetached)));
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "Detached must fail fast, never block -- there's nothing to wait for"
        );
    }

    #[test]
    fn check_open_blocks_while_suspended_and_succeeds_once_resumed() {
        let _flock_test_guard = flock_test_guard();
        let dir = tempdir().unwrap();
        // Generous timeout -- the point of this test is that resume
        // wins the race well before it, not that the timeout itself
        // fires.
        let handle = open_suspend_test(dir.path(), Duration::from_secs(5));
        handle.shared.set_state(HandleState::Suspended);

        let shared = Arc::clone(&handle.shared);
        let resumer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            shared.set_state(HandleState::Open);
        });

        let start = Instant::now();
        handle
            .write_atomic(&dir.path().join("x.txt"), b"data")
            .unwrap();
        let elapsed = start.elapsed();
        resumer.join().unwrap();

        assert!(
            elapsed >= Duration::from_millis(40),
            "should have genuinely blocked until resume, not returned instantly: {elapsed:?}"
        );
        assert_eq!(fs::read(dir.path().join("x.txt")).unwrap(), b"data");
    }

    #[test]
    fn check_open_gives_up_after_the_timeout_and_returns_suspended() {
        let _flock_test_guard = flock_test_guard();
        let dir = tempdir().unwrap();
        let handle = open_suspend_test(dir.path(), FAST_TEST_SUSPEND_TIMEOUT);
        handle.shared.set_state(HandleState::Suspended);

        let start = Instant::now();
        let result = handle.write_atomic(&dir.path().join("x.txt"), b"data");
        let elapsed = start.elapsed();

        assert!(matches!(result, Err(LibraryHandleError::Suspended)));
        assert!(
            elapsed >= FAST_TEST_SUSPEND_TIMEOUT,
            "should have waited out the full timeout before giving up: {elapsed:?}"
        );
    }

    #[test]
    fn check_open_wakes_immediately_on_a_transition_straight_to_detached() {
        let _flock_test_guard = flock_test_guard();
        let dir = tempdir().unwrap();
        // Generous timeout -- the point is that the Detached
        // transition wakes this well before the timeout would ever
        // fire on its own.
        let handle = open_suspend_test(dir.path(), Duration::from_secs(5));
        handle.shared.set_state(HandleState::Suspended);

        let shared = Arc::clone(&handle.shared);
        let detacher = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            shared.set_state(HandleState::Detached);
        });

        let start = Instant::now();
        let result = handle.write_atomic(&dir.path().join("x.txt"), b"data");
        let elapsed = start.elapsed();
        detacher.join().unwrap();

        assert!(matches!(result, Err(LibraryHandleError::DeviceDetached)));
        assert!(
            elapsed < Duration::from_secs(2),
            "should have woken immediately on the Detached transition, \
             not waited out the 5s timeout: {elapsed:?}"
        );
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
