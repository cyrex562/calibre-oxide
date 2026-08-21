//! `LibraryHandle` (issue #93, phase 1): the single gateway every
//! durable write to a library folder is meant to go through, per
//! `docs/FAULT_TOLERANCE.md`.
//!
//! # Scope of this phase
//!
//! The design doc describes a large system: storage-tier
//! classification, a writer lock, atomic write primitives, a
//! BLAKE3-chained write-ahead journal with crash recovery, OS device-
//! removal notifications, OS sleep/resume notifications, two-phase
//! network-storage writes, and (per its own "definition of done") a
//! crate-wide retrofit so no other file in this crate touches a
//! library path with a raw `fs::write`/`fs::rename`. That is
//! realistically several more PRs' worth of work -- this one, phase
//! 1, ships the foundation the rest builds on, real and independently
//! useful on its own:
//!
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
//!   descriptor closes for any reason) -- exactly the "don't leave a
//!   stale lock after a crash" property the design doc wants.
//! - Atomic write primitives (§2 step 3, minus the journal wrapper):
//!   [`LibraryHandle::write_atomic`] (write-temp / fsync-temp / rename
//!   / fsync-parent-directory) and [`LibraryHandle::rename_atomic`]
//!   (POSIX `rename` is already atomic; this adds the fsync-parent-
//!   directory step upstream's own discipline requires). Directory
//!   fsync is real on Unix (`File::open` on a directory + `sync_all`,
//!   a standard POSIX technique); a no-op on Windows, which has a
//!   different durability model for directory entries (the design
//!   doc's own Windows answer is `MoveFileExW` with
//!   `MOVEFILE_WRITE_THROUGH`, not directory fsync -- not implemented
//!   here, same disclosed Windows gap as tier classification).
//! - Lifecycle states ([`HandleState`]): `Open`/`Suspended`/`Detached`
//!   exist as real types with a real `Detached` error contract
//!   ([`LibraryHandleError::DeviceDetached`]), but nothing in this
//!   phase ever transitions a handle away from `Open` -- that's what
//!   the device-notification (§4) and power-state (§5) phases wire
//!   up. A `LibraryHandle` today is always `Open` for its whole
//!   lifetime.
//! - Fault-injection testing of the write-atomic sequence
//!   (`docs/FAULT_TOLERANCE.md`'s "testable invariants"): rather than
//!   pulling in the `fail` crate (an extra dependency for a single
//!   file's tests), [`LibraryHandle`] has an internal, always-compiled
//!   (but only ever exercised by tests) `write_atomic_impl` that takes
//!   an optional fault-injection point and simulates a crash
//!   immediately after each step of the sequence; the tests assert
//!   the one invariant `write_atomic` itself can guarantee without a
//!   journal: the *original* target file is left completely untouched
//!   by a failure at any point before the rename commits.
//!
//! # Not in this phase (disclosed, tracked as later work under #93)
//!
//! - The write-ahead journal and crash-recovery replay (§2 steps 1-2,
//!   4-5) -- `write_atomic`/`rename_atomic` are atomic *individual*
//!   operations already (real, useful today), but nothing here yet
//!   records a journal entry beforehand or replays one on reopen.
//!   BLAKE3 checksums (§8) are part of this same later phase.
//! - Device-removal notifications (§4) and the `Detached` transition.
//! - Sleep/resume notifications (§5) and the `Suspended` transition.
//! - Network-storage two-phase writes (§6) -- [`StorageTier::Network`]
//!   is detected and stored, but nothing yet changes behavior based on
//!   it.
//! - **The crate-wide retrofit.** Every existing `fs::write`/
//!   `fs::rename`/`fs::copy` call against a library path elsewhere in
//!   this crate (`cache.rs`, `restore.rs`, `notes/connection.rs`,
//!   `covers.rs`, `adding.rs`, and more) still calls `std::fs`
//!   directly, not through this handle. Retrofitting all of them is
//!   its own large, separately-reviewable change -- doing it in the
//!   same pass as introducing the primitive would combine two very
//!   different kinds of risk (new-code-correctness and regressing
//!   many already-shipped, tested write paths at once).
//! - Windows implementations of tier classification and directory
//!   durability (see above) -- this workspace has no way to compile-
//!   check or test Windows-specific code, so rather than ship
//!   plausible-looking but unverified FFI, both fall back to a
//!   disclosed, conservative default.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use thiserror::Error;

use crate::constants::{LIBRARY_HANDLE_DIR_NAME, WRITER_LOCK_FILE_NAME};

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
/// in this phase transitions a handle out of `Open` -- see this
/// module's doc comment.
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
    /// hidden in the handle"); reserved -- nothing in this phase ever
    /// produces this yet (see module doc).
    #[error("the library's storage device has been detached")]
    DeviceDetached,
    /// Reserved for the power-state phase (§5).
    #[error("the library handle is suspended")]
    Suspended,
    /// Reserved for the checksum phase (§8).
    #[error("data corruption detected: {0}")]
    Corruption(String),
}

/// The single gateway every durable write to a library folder is
/// meant to go through. See this module's doc comment for exactly
/// what's real in this phase.
pub struct LibraryHandle {
    library_path: PathBuf,
    tier: StorageTier,
    state: Mutex<HandleState>,
    /// Held open for the handle's whole lifetime -- the OS releases
    /// the advisory lock automatically when this (and every other
    /// clone of the fd, which there are none of here) closes, which
    /// includes process crash/kill. This is what gives "no stale lock
    /// left behind after a crash" for free.
    _lock_file: File,
}

impl LibraryHandle {
    /// Opens (creating `<library>/.calibre-oxide/` if needed) and
    /// acquires the exclusive writer lock. Fails with
    /// [`LibraryHandleError::AlreadyLocked`] if another `LibraryHandle`
    /// (in this or another process) already holds it -- no blocking,
    /// no retry loop, matching §7's "one writer per library" rule.
    pub fn open(library_path: &Path) -> Result<Self, LibraryHandleError> {
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

        Ok(LibraryHandle {
            library_path: library_path.to_path_buf(),
            tier: classify_storage_tier(library_path),
            state: Mutex::new(HandleState::Open),
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

    /// Port of §2 step 3: write-temp / fsync-temp / rename / fsync-
    /// parent-directory. `target` must be an absolute path (or at
    /// least caller-resolved -- this doesn't re-root it under
    /// `library_path`, callers do that).
    pub fn write_atomic(&self, target: &Path, bytes: &[u8]) -> Result<(), LibraryHandleError> {
        self.write_atomic_impl(target, bytes, None)
    }

    /// Port of §2 step 3's rename half, for callers that already have
    /// the payload written to its final form elsewhere (e.g. a format
    /// file copied by a caller, then atomically published into place)
    /// -- POSIX `rename` is already atomic; this adds the durability-
    /// relevant fsync of the parent director(ies) upstream's
    /// discipline requires.
    pub fn rename_atomic(&self, from: &Path, to: &Path) -> Result<(), LibraryHandleError> {
        self.check_open()?;
        fs::rename(from, to)?;
        fsync_dir(from.parent())?;
        if to.parent() != from.parent() {
            fsync_dir(to.parent())?;
        }
        Ok(())
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
        let parent = target.parent();
        if let Some(parent) = parent {
            fs::create_dir_all(parent)?;
        }
        let tmp_name = format!(
            "{}.tmp-{}",
            target
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file"),
            uuid::Uuid::new_v4()
        );
        let tmp_path = match parent {
            Some(p) => p.join(&tmp_name),
            None => PathBuf::from(&tmp_name),
        };

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
        Ok(())
    }
}

/// Test-only fault-injection points within [`LibraryHandle::write_atomic_impl`]
/// (always compiled, since it's a plain enum with no runtime cost --
/// avoids `#[cfg(test)]` on a function-call argument, which isn't
/// stable syntax).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailPoint {
    WriteTemp,
    FsyncTemp,
    Rename,
}

fn simulated_crash() -> LibraryHandleError {
    LibraryHandleError::Io(io::Error::other(
        "simulated crash (test-only fault injection)",
    ))
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

    // --- fault-injection: docs/FAULT_TOLERANCE.md's "kill process at
    // random point" testable invariant, for write_atomic specifically
    // (the one operation in this phase real enough to have a
    // meaningful "crash mid-sequence" story) {{{

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
        // of whether the subsequent directory-fsync step completes --
        // this is exactly why write-temp-then-rename is the right
        // shape: the commit point is a single atomic filesystem
        // operation, not a multi-step window.
        let dir = tempdir().unwrap();
        let handle = LibraryHandle::open(dir.path()).unwrap();
        let target = dir.path().join("metadata.opf");
        fs::write(&target, b"original").unwrap();

        let err = handle.write_atomic_impl(&target, b"new", Some(FailPoint::Rename));
        assert!(err.is_err());
        assert_eq!(fs::read(&target).unwrap(), b"new");
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
