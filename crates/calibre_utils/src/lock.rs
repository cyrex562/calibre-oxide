//! Port of `old_src/src/calibre/utils/lock.py`, narrowed to the Linux
//! path (issue #62's own triage) -- the Windows (`msvcrt`/`winutil`)
//! and macOS/BSD (`/var/lock`-style lock-file fallback) branches are
//! platform-specific code this Linux dev environment can't verify, so
//! they're not ported here.
//!
//! # Scope
//!
//! Real: [`lock_file`] (an exclusive, non-blocking `flock` with
//! retry-on-transient-error, matching `unix_open`/`unix_retry`/
//! `retry_for_a_time`/`lock_file`) and [`create_single_instance_mutex`]
//! (a single-instance-app guard via a Linux abstract-namespace Unix
//! domain socket, matching the `islinux` branch of upstream's own
//! `create_single_instance_mutex`).
//!
//! `ExclusiveFile`/`SingleInstance` (upstream's context-manager
//! wrappers around `lock_file`/`create_single_instance_mutex`) aren't
//! ported as separate types: [`std::fs::File`]'s own `Drop` already
//! releases the `flock` on close, and [`SingleInstanceGuard`]'s
//! `Drop` already releases the abstract socket -- Rust's ownership
//! model makes the wrapper types Python needs for `with` blocks
//! redundant here.
//!
//! `singleinstance()`'s `atexit.register(release_mutex)` becomes the
//! caller's job: hold the returned [`SingleInstanceGuard`] for as
//! long as the "single instance" should be considered held (e.g. in
//! a `static`/`OnceLock`, or just a long-lived local), and it
//! releases on drop.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::linux::net::SocketAddrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{SocketAddr, UnixListener};
use std::path::Path;
use std::time::Duration;

use crate::monotonic::monotonic;

/// `S_IRUSR | S_IWUSR | S_IRGRP | S_IROTH` -- upstream's own
/// non-Windows `excl_file_mode`.
const EXCL_FILE_MODE: u32 = 0o644;

fn unix_retry(err: &io::Error) -> bool {
    matches!(err.raw_os_error(), Some(libc::EACCES) | Some(libc::EAGAIN) | Some(libc::ENOLCK) | Some(libc::EINTR))
}

/// Port of `lock_file`: opens `path` (creating it if missing) and
/// takes an exclusive, non-blocking `flock`, retrying on transient
/// errors (matching `unix_retry`) for up to `timeout`, sleeping
/// `sleep_time` between attempts. The file is closed (and the lock
/// released) automatically when the returned `File` is dropped.
pub fn lock_file(path: &Path, timeout: Duration, sleep_time: Duration) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(EXCL_FILE_MODE)
        .custom_flags(libc::O_CLOEXEC)
        .open(path)?;

    let limit = monotonic() + timeout.as_secs_f64();
    loop {
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc == 0 {
            return Ok(file);
        }
        let err = io::Error::last_os_error();
        if !unix_retry(&err) || monotonic() > limit {
            return Err(err);
        }
        std::thread::sleep(sleep_time);
    }
}

/// Releases a single-instance mutex ([`create_single_instance_mutex`])
/// when dropped.
pub struct SingleInstanceGuard {
    _listener: UnixListener,
}

/// Port of the Linux branch of `create_single_instance_mutex`:
/// claims a Linux abstract-namespace Unix domain socket named after
/// `app_name`/`name`/(optionally) the calling user's effective uid,
/// as a single-instance-per-machine (or per-user) guard. Returns
/// `None` if another process already holds it (matching upstream's
/// own `None` return on `EADDRINUSE`).
pub fn create_single_instance_mutex(app_name: &str, name: &str, per_user: bool) -> Option<SingleInstanceGuard> {
    let user_part = if per_user { unsafe { libc::geteuid() }.to_string() } else { String::new() };
    let full_name = format!("{app_name}-singleinstance-{user_part}-{name}").replace(' ', "_");
    let addr = SocketAddr::from_abstract_name(full_name.as_bytes()).ok()?;
    UnixListener::bind_addr(&addr).ok().map(|listener| SingleInstanceGuard { _listener: listener })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locks_a_file_exclusively() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.lock");
        let _f = lock_file(&path, Duration::from_secs(1), Duration::from_millis(10)).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn a_second_attempt_times_out_while_the_first_holds_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.lock");
        let _first = lock_file(&path, Duration::from_secs(1), Duration::from_millis(10)).unwrap();

        let start = std::time::Instant::now();
        let result = lock_file(&path, Duration::from_millis(200), Duration::from_millis(20));
        assert!(result.is_err(), "a second exclusive lock attempt should fail while the first is held");
        assert!(start.elapsed() >= Duration::from_millis(150), "should have actually retried for close to the timeout");
    }

    #[test]
    fn the_lock_is_released_when_the_file_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.lock");
        {
            let _f = lock_file(&path, Duration::from_secs(1), Duration::from_millis(10)).unwrap();
        }
        // Should succeed immediately now that the first guard is gone.
        let _f2 = lock_file(&path, Duration::from_millis(200), Duration::from_millis(10)).unwrap();
    }

    #[test]
    fn single_instance_mutex_is_exclusive_and_released_on_drop() {
        // A random-ish name so parallel test runs don't collide.
        let name = format!("test-{}", std::process::id());
        let guard = create_single_instance_mutex("calibre-oxide-test", &name, true);
        assert!(guard.is_some(), "first claim should succeed");

        let second = create_single_instance_mutex("calibre-oxide-test", &name, true);
        assert!(second.is_none(), "second claim should fail while the first is held");

        drop(guard);
        let third = create_single_instance_mutex("calibre-oxide-test", &name, true);
        assert!(third.is_some(), "claim should succeed again once the first guard is dropped");
    }
}
