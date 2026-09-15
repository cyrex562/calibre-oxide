//! Port of `calibre.utils.safe_atexit` (issue #467): crash-safe
//! cleanup via a persistent worker subprocess.
//!
//! # The real mechanism
//!
//! A single, lazily-spawned worker process (`safe_atexit_worker`,
//! `src/bin/safe_atexit_worker.rs`) reads newline-delimited JSON
//! commands from its stdin. `rmtree`/
//! `unlink` commands are not acted on immediately -- they are queued,
//! and only run when the worker's own stdin hits EOF. `run_program`
//! commands spawn immediately.
//!
//! That queue-until-EOF design is what makes this crash-safe: when
//! *this* process (the caller) exits for any reason -- a clean
//! `exit()`, a panic, or a hard kill -- the OS reclaims every file
//! descriptor it held, including the write end of the worker's stdin
//! pipe. The worker sees that as EOF on stdin regardless of how the
//! caller went away, runs every queued cleanup action, and exits.
//! Nothing about the crash-safety property depends on the caller
//! cooperating; it depends only on the OS closing pipe fds on process
//! exit, which it always does.
//!
//! [`shutdown_worker`] is the *graceful*-exit fast path: it closes the
//! worker's stdin itself and waits (with a timeout, then a forced
//! kill) for the worker to actually finish, so a caller that exits
//! normally doesn't return control to whoever's waiting on it before
//! cleanup has actually happened. Skipping it is still safe -- the
//! worker still gets EOF and still runs its queue -- just
//! asynchronously from the caller's own exit.
//!
//! # Disclosed narrowings
//!
//! - **No `atexit`-based auto-shutdown.** Upstream registers
//!   `close_worker` via `atexit.register` so *every* process that ever
//!   calls into this module gets the graceful-wait-then-kill behavior
//!   automatically, with no explicit call needed. Rust has no
//!   equivalent that can capture the worker's own state the way
//!   Python's closure-based `atexit.register(close_worker, worker)`
//!   does -- same gap `tdir_in_cache.rs` already discloses for the
//!   same reason. Call [`shutdown_worker`] yourself at your own
//!   natural shutdown point if you want the synchronous wait; skipping
//!   it only affects *when* cleanup runs, never *whether* it does.
//! - **`os.path.abspath`, not full symlink resolution.** [`make_absolute`]
//!   joins a relative path onto the current directory without
//!   resolving symlinks (matching upstream exactly) -- using
//!   `Path::canonicalize` here would be a real behavior change for
//!   deletion: it would `rmtree`/`unlink` a symlink's *target*, not
//!   the symlink's own location.
//! - **Windows' `remove_dir`'s ten-attempt retry-with-sleep loop
//!   (working around another program transiently holding one of the
//!   files open) isn't ported.** Unverifiable on this Linux-only
//!   toolchain, same reasoning as every other Windows-only gap in this
//!   crate (issues #78/#79/#258). Both platforms get the same
//!   best-effort, ignore-errors removal upstream's own Unix
//!   `remove_dir` uses.
//! - **`sanitize_env_vars` isn't ported.** It exists to stop a
//!   PyInstaller-frozen calibre binary's own bundled shared libraries
//!   (`LD_LIBRARY_PATH`, `OPENSSL_MODULES`, ...) from leaking into a
//!   `run_program`-launched external tool's environment. A native Rust
//!   binary has no bundled shared-library directory to leak in the
//!   first place -- not a narrowing, the upstream concern doesn't
//!   apply here.
//! - **`reset_dll_dir`/Windows DLL-search-path reset in the worker's
//!   own startup isn't ported** -- same Windows-unverifiable reasoning.
//! - **No current caller.** Nothing in this port creates temp files
//!   through a `ptempfile.rs`-equivalent wrapper yet -- they use the
//!   `tempfile` crate directly (`Drop`-based cleanup on a clean exit,
//!   the same guarantee Python's own plain `tempfile` module gives,
//!   with no crash-safety net). This module is a real, ready-to-use
//!   primitive for any future caller that needs one; it was not wired
//!   into anything as part of this issue.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

struct Worker {
    child: Child,
}

static WORKER: OnceLock<Mutex<Option<Worker>>> = OnceLock::new();

fn worker_slot() -> &'static Mutex<Option<Worker>> {
    WORKER.get_or_init(|| Mutex::new(None))
}

/// Port of `os.path.abspath`: joins a relative path onto the current
/// directory without resolving symlinks. See this module's doc for
/// why that (not [`Path::canonicalize`]) is the correct choice here.
fn make_absolute(path: &Path) -> io::Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Ok(std::env::current_dir()?.join(path))
}

/// Port of `ensure_worker`: spawns the worker (`worker_binary`, a
/// compiled `safe_atexit_worker`) the first time any of this module's
/// functions is called; a no-op afterward. All callers in one process
/// share the one worker, so they must agree on `worker_binary` --
/// whichever call happens first decides it for the process's lifetime.
fn ensure_worker(worker_binary: &Path) -> io::Result<()> {
    let mut slot = worker_slot().lock().unwrap();
    if slot.is_none() {
        let child = Command::new(worker_binary).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
        *slot = Some(Worker { child });
    }
    Ok(())
}

fn send_command(worker_binary: &Path, action: &str, payload: serde_json::Value) -> io::Result<()> {
    ensure_worker(worker_binary)?;
    let mut slot = worker_slot().lock().unwrap();
    let worker = slot.as_mut().expect("ensure_worker just populated this");
    let stdin = worker.child.stdin.as_mut().expect("spawned with a piped stdin");
    let line = serde_json::json!({"action": action, "payload": payload}).to_string();
    stdin.write_all(line.as_bytes())?;
    stdin.write_all(b"\n")?;
    stdin.flush()
}

/// Port of `remove_folder_atexit`: queue `path` (a directory) for
/// recursive removal when the worker process itself exits -- on a
/// clean [`shutdown_worker`] call, or on this process's own crash or
/// kill (the worker sees EOF either way).
pub fn remove_folder_atexit(worker_binary: &Path, path: &Path) -> io::Result<()> {
    let abs = make_absolute(path)?;
    send_command(worker_binary, "rmtree", serde_json::json!(abs.to_string_lossy()))
}

/// Port of `remove_file_atexit`: same as [`remove_folder_atexit`], for
/// a single file.
pub fn remove_file_atexit(worker_binary: &Path, path: &Path) -> io::Result<()> {
    let abs = make_absolute(path)?;
    send_command(worker_binary, "unlink", serde_json::json!(abs.to_string_lossy()))
}

/// Port of `run_program_now`: launches `cmdline` from the worker
/// process immediately (not queued), so it keeps running even if this
/// process exits right after calling this.
pub fn run_program_now(worker_binary: &Path, cmdline: &[String]) -> io::Result<()> {
    send_command(worker_binary, "run_program", serde_json::json!(cmdline))
}

/// Port of `close_worker`, called explicitly rather than via
/// `atexit.register` (see this module's doc). Closes the worker's
/// stdin (EOF -- it runs every queued cleanup action and exits),
/// waits up to `timeout` for it to exit on its own, then kills it. A
/// no-op if no worker was ever spawned.
pub fn shutdown_worker(timeout: Duration) -> io::Result<()> {
    let mut slot = worker_slot().lock().unwrap();
    let Some(mut worker) = slot.take() else {
        return Ok(());
    };
    drop(worker.child.stdin.take());

    let deadline = Instant::now() + timeout;
    loop {
        if worker.child.try_wait()?.is_some() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = worker.child.kill();
    worker.child.wait()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// `WORKER` is one real, process-wide global (matching upstream's
    /// own module-level singleton) -- these tests deliberately manage
    /// it directly (spawning it, tearing it down mid-test to simulate
    /// a crash), which races destructively against Rust's default
    /// parallel test execution. Serializes them, same shape as
    /// `library_handle.rs`'s `FLOCK_TEST_SERIALIZE`/`flock_test_guard`.
    static TEST_SERIALIZE: Mutex<()> = Mutex::new(());
    fn test_guard() -> std::sync::MutexGuard<'static, ()> {
        TEST_SERIALIZE.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn worker_binary() -> Option<PathBuf> {
        let candidate = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/safe_atexit_worker").canonicalize().ok()?;
        candidate.exists().then_some(candidate)
    }

    fn wait_until_gone(path: &Path, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if !path.exists() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        !path.exists()
    }

    #[test]
    fn a_queued_removal_runs_on_graceful_shutdown() {
        let _guard = test_guard();
        let Some(worker_binary) = worker_binary() else {
            eprintln!("skipping: safe_atexit_worker not built (run `cargo build -p calibre_utils` first)");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("cleanup-me");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("f"), b"x").unwrap();
        assert!(target.exists());

        remove_folder_atexit(&worker_binary, &target).unwrap();
        // Not removed yet -- it's queued, not immediate.
        assert!(target.exists());

        shutdown_worker(Duration::from_secs(5)).unwrap();
        assert!(!target.exists());
    }

    #[test]
    fn a_queued_removal_still_runs_if_the_caller_is_killed_not_shut_down() {
        // The crash-safety property itself: simulate "this process
        // died without calling shutdown_worker" by just dropping the
        // worker handle without an explicit graceful close. Rust
        // doesn't give us a way to SIGKILL our own test process and
        // keep asserting afterward, so this drives the same
        // EOF-on-fd-reclaim mechanism a real crash relies on by
        // explicitly closing the pipe out from under the worker
        // without going through the documented shutdown path, then
        // polling for the worker to notice and finish on its own.
        let _guard = test_guard();
        let Some(worker_binary) = worker_binary() else {
            eprintln!("skipping: safe_atexit_worker not built (run `cargo build -p calibre_utils` first)");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("crash-cleanup-me");
        std::fs::write(&target, b"x").unwrap();
        assert!(target.exists());

        remove_file_atexit(&worker_binary, &target).unwrap();
        assert!(target.exists());

        // Reach into the slot directly and drop the child's stdin
        // (closing our end of the pipe) without following the
        // documented wait/kill shutdown sequence -- this is exactly
        // what the OS does to every fd a process held the instant
        // that process (crashed or not) stops existing.
        {
            let mut slot = worker_slot().lock().unwrap();
            let worker = slot.as_mut().unwrap();
            worker.child.stdin.take();
        }

        assert!(wait_until_gone(&target, Duration::from_secs(5)));

        // Clean up the now-exited worker so later tests get a fresh one.
        let mut slot = worker_slot().lock().unwrap();
        if let Some(mut worker) = slot.take() {
            let _ = worker.child.wait();
        }
    }

    #[test]
    fn run_program_now_launches_immediately_not_queued() {
        let _guard = test_guard();
        let Some(worker_binary) = worker_binary() else {
            eprintln!("skipping: safe_atexit_worker not built (run `cargo build -p calibre_utils` first)");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("ran");
        assert!(!marker.exists());

        run_program_now(&worker_binary, &["touch".to_string(), marker.to_string_lossy().to_string()]).unwrap();
        // `run_program` isn't queued -- it should appear well before
        // any shutdown, without calling shutdown_worker at all.
        let appeared = {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut ok = false;
            while Instant::now() < deadline {
                if marker.exists() {
                    ok = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            ok
        };
        assert!(appeared, "run_program_now's marker file never appeared");
    }

    #[test]
    fn make_absolute_does_not_resolve_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let real_target = dir.path().join("real_target");
        std::fs::create_dir(&real_target).unwrap();
        let link = dir.path().join("a_link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_target, &link).unwrap();
        #[cfg(unix)]
        {
            let abs = make_absolute(&link).unwrap();
            assert_eq!(abs, link, "abspath must not resolve the symlink");
            assert_ne!(abs, real_target.canonicalize().unwrap());
        }
    }
}
