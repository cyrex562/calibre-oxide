//! Port of `old_src/src/calibre/utils/tdir_in_cache.py`, narrowed to
//! the Linux path (matching [`crate::lock`]'s own narrowing) --
//! creates a temp directory under `cache_dir()` that survives a crash
//! well enough to be cleaned up automatically the next time the
//! application starts, using a per-directory `fcntl` record lock
//! (upstream's own `fcntl.lockf`, a different primitive from
//! [`crate::lock::lock_file`]'s `flock` -- deliberately not shared,
//! matching upstream's own separate implementation) to tell a
//! still-running owner apart from one that crashed and left its temp
//! dir behind.
//!
//! # Not ported: the `atexit` registration
//!
//! Upstream registers `remove_tdir` to run at process exit via
//! `atexit.register`, so a *clean* shutdown removes the temp dir
//! immediately rather than waiting for the next startup's sweep.
//! Rust has no direct equivalent that can capture owned state the way
//! `atexit.register(remove_tdir, tdir, lock_data)` does. [`tdir_in_cache`]
//! returns the lock [`std::fs::File`] alongside the path instead --
//! callers that want eager cleanup on a clean exit should call
//! [`remove_tdir`] themselves (e.g. from their own shutdown path);
//! callers that don't are still safe, since a leaked tdir is exactly
//! what [`clean_tdirs_in`] sweeps up on the next call, by design.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::constants::cache_dir;
use crate::monotonic::monotonic;

const TDIR_LOCK: &str = "tdir-lock";

fn eintr_retry<F: FnMut() -> libc::c_int>(mut f: F) -> io::Result<()> {
    loop {
        if f() == 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::EINTR) {
            return Err(err);
        }
    }
}

fn record_lock(f: &File, l_type: libc::c_short) -> io::Result<()> {
    let mut fl: libc::flock = unsafe { std::mem::zeroed() };
    fl.l_type = l_type;
    fl.l_whence = libc::SEEK_SET as libc::c_short;
    fl.l_start = 0;
    fl.l_len = 0;
    eintr_retry(|| unsafe { libc::fcntl(f.as_raw_fd(), libc::F_SETLK, &fl) })
}

/// Port of `lock_tdir`: an exclusive, non-blocking `fcntl` record
/// lock on `path/tdir-lock` (created if missing).
pub fn lock_tdir(path: &Path) -> io::Result<File> {
    let f = OpenOptions::new().write(true).create(true).truncate(true).open(path.join(TDIR_LOCK))?;
    record_lock(&f, libc::F_WRLCK as libc::c_short)?;
    Ok(f)
}

/// Port of `unlock_file`.
pub fn unlock_file(f: File) -> io::Result<()> {
    record_lock(&f, libc::F_UNLCK as libc::c_short)
}

/// Port of `remove_tdir`: releases the lock, then removes the
/// directory tree.
pub fn remove_tdir(path: &Path, lock_file: File) -> io::Result<()> {
    let _ = unlock_file(lock_file);
    fs::remove_dir_all(path)
}

/// Port of `is_tdir_locked`: whether another process currently holds
/// `path`'s lock (i.e. `path` is still in active use, not just left
/// behind by a crash).
pub fn is_tdir_locked(path: &Path) -> bool {
    let Ok(f) = OpenOptions::new().write(true).create(true).truncate(true).open(path.join(TDIR_LOCK)) else {
        return false;
    };
    if record_lock(&f, libc::F_WRLCK as libc::c_short).is_err() {
        return true;
    }
    let _ = record_lock(&f, libc::F_UNLCK as libc::c_short);
    false
}

/// Port of `tdirs_in`: every direct subdirectory of `base` (empty if
/// `base` itself doesn't exist).
fn tdirs_in(base: &Path) -> Vec<PathBuf> {
    match fs::read_dir(base) {
        Ok(entries) => entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(_) => Vec::new(),
    }
}

/// Port of `clean_tdirs_in`: removes every unlocked (i.e.
/// crash-orphaned) subdirectory of `base`.
pub fn clean_tdirs_in(base: &Path) {
    for tdir in tdirs_in(base) {
        if !is_tdir_locked(&tdir) {
            let _ = fs::remove_dir_all(&tdir);
        }
    }
}

/// Port of `retry_lock_tdir`.
pub fn retry_lock_tdir(path: &Path, timeout: Duration, sleep: Duration) -> io::Result<File> {
    let limit = monotonic() + timeout.as_secs_f64();
    loop {
        match lock_tdir(path) {
            Ok(f) => return Ok(f),
            Err(e) => {
                // Matches upstream's own `monotonic() - st > timeout` check.
                if monotonic() > limit {
                    return Err(e);
                }
                std::thread::sleep(sleep);
            }
        }
    }
}

fn scanned_dirs() -> &'static Mutex<HashSet<PathBuf>> {
    static SCANNED: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    SCANNED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Port of `tdir_in_cache`: creates a temp dir inside
/// `cache_dir()/base`. The created dir is robust against application
/// crashes -- it gets cleaned up the next time [`tdir_in_cache`] runs
/// for the same `base`, even if a previous run crashed before
/// removing it. See the module doc for the one behavioral narrowing
/// (no `atexit`-registered eager cleanup).
///
/// Returns the final directory (a fresh, empty `a/` subdirectory of
/// the actual temp dir) and the open lock file guarding *that*
/// specific temp dir -- pass both to [`remove_tdir`] for eager
/// cleanup, or just drop the lock file and let the next
/// [`tdir_in_cache`] call for this `base` sweep it up.
pub fn tdir_in_cache(base: &str) -> io::Result<(PathBuf, File)> {
    let cache_root = cache_dir().canonicalize().unwrap_or_else(|_| cache_dir());
    tdir_in(&cache_root, base)
}

/// [`tdir_in_cache`]'s real logic, taking the cache root explicitly
/// rather than always reading it from [`cache_dir`] -- lets tests
/// point it at a temp directory instead of the process-wide (and
/// `lazy_static`-cached, so not reliably env-var-overridable after
/// first use) real cache directory.
fn tdir_in(cache_root: &Path, base: &str) -> io::Result<(PathBuf, File)> {
    let b = cache_root.join(base);
    fs::create_dir_all(&b)?;

    let global_lock = retry_lock_tdir(&b, Duration::from_secs(30), Duration::from_millis(100))?;

    let result = (|| -> io::Result<(PathBuf, File)> {
        {
            let mut scanned = scanned_dirs().lock().unwrap();
            if scanned.insert(b.clone()) {
                clean_tdirs_in(&b);
            }
        }
        let tdir = tempfile::Builder::new().tempdir_in(&b)?.keep();
        let lock_data = lock_tdir(&tdir)?;
        let inner = tdir.join("a");
        fs::create_dir(&inner)?;
        Ok((inner, lock_data))
    })();

    let _ = unlock_file(global_lock);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tdirs_in_lists_only_directories_and_tolerates_a_missing_base() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("a")).unwrap();
        fs::write(dir.path().join("not-a-dir"), b"x").unwrap();
        let found = tdirs_in(dir.path());
        assert_eq!(found, vec![dir.path().join("a")]);

        assert!(tdirs_in(&dir.path().join("does-not-exist")).is_empty());
    }

    /// `fcntl` record locks are *process*-scoped, not per-file-
    /// descriptor: a second `F_SETLK` from the *same* process just
    /// succeeds (replacing its own lock), it never reports
    /// `EAGAIN`/`EACCES`. So the only correct way to observe
    /// `is_tdir_locked() == true` is from a genuinely separate
    /// process. This spawns one via `python3`'s own `fcntl.lockf` --
    /// the exact mechanism upstream's real Python implementation
    /// uses -- rather than mocking anything. Returns the child
    /// (blocked reading stdin) once it has confirmed the lock is
    /// held; write a line to its stdin and `wait()` it to release.
    fn spawn_lock_holder(lock_path: &Path) -> std::process::Child {
        use std::io::{BufRead, BufReader};

        let mut child = std::process::Command::new("python3")
            .arg("-c")
            .arg(
                "import fcntl, sys
f = open(sys.argv[1], 'w')
fcntl.lockf(f.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
print('locked', flush=True)
sys.stdin.readline()
",
            )
            .arg(lock_path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("python3 must be available to run this test");

        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
        assert_eq!(line.trim(), "locked", "the helper process should have confirmed it holds the lock");
        child
    }

    fn release_lock_holder(mut child: std::process::Child) {
        use std::io::Write;
        child.stdin.take().unwrap().write_all(b"\n").unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn a_locked_tdir_is_reported_locked_and_an_unlocked_one_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let tdir = dir.path().join("t1");
        fs::create_dir(&tdir).unwrap();

        assert!(!is_tdir_locked(&tdir), "nothing holds the lock yet");

        let holder = spawn_lock_holder(&tdir.join(TDIR_LOCK));
        assert!(is_tdir_locked(&tdir), "a real other process holds the lock");
        release_lock_holder(holder);

        assert!(!is_tdir_locked(&tdir), "released once the other process exits");
    }

    #[test]
    fn clean_tdirs_in_removes_only_unlocked_subdirectories() {
        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        let orphan = dir.path().join("orphan");
        fs::create_dir(&locked).unwrap();
        fs::create_dir(&orphan).unwrap();
        let holder = spawn_lock_holder(&locked.join(TDIR_LOCK));

        clean_tdirs_in(dir.path());

        assert!(locked.exists(), "a still-locked (in-use) tdir must survive");
        assert!(!orphan.exists(), "an unlocked (crash-orphaned) tdir must be swept");
        release_lock_holder(holder);
    }

    #[test]
    fn tdir_in_cache_creates_a_usable_directory_guarded_by_its_own_lock_file() {
        let cache = tempfile::tempdir().unwrap();
        let (tdir, lock) = tdir_in(cache.path(), "calibre-oxide-test").unwrap();
        assert!(tdir.is_dir());
        assert!(tdir.ends_with("a"));
        assert!(tdir.parent().unwrap().join(TDIR_LOCK).exists());
        drop(lock);
    }

    #[test]
    fn a_second_call_for_the_same_base_sweeps_orphaned_tdirs_from_the_first() {
        let cache = tempfile::tempdir().unwrap();
        let (tdir1, lock1) = tdir_in(cache.path(), "calibre-oxide-test-sweep").unwrap();
        let orphaned_parent = tdir1.parent().unwrap().to_path_buf();
        // Simulate a crash: drop the lock without removing the dir.
        drop(lock1);
        assert!(orphaned_parent.exists());

        // A real second call would notice `base` is already in the
        // process-wide `scanned` set and skip re-sweeping it -- call
        // clean_tdirs_in directly to exercise the sweep itself instead.
        clean_tdirs_in(&cache.path().join("calibre-oxide-test-sweep"));
        assert!(!orphaned_parent.exists(), "the orphaned tdir from the crashed run should be swept");
    }
}
