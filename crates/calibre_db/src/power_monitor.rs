//! Port of `docs/FAULT_TOLERANCE.md` §5 (issue #93): real sleep/
//! resume handling for [`crate::library_handle::LibraryHandle`], via
//! `systemd-logind`'s `PrepareForSleep` D-Bus signal -- real, verified
//! by connecting to this machine's actual system bus during
//! development (see below) -- plus a real "delay"-type sleep
//! inhibitor lock, without which this crate's suspend-prep work
//! (releasing the writer lock, fsyncing) would just be racing against
//! the actual suspend rather than guaranteed to finish first.
//!
//! # Verified, not just implemented from documentation
//!
//! Two things were confirmed against this machine's real
//! `systemd-logind` before writing any of this module's real code
//! (same "probe before trusting" discipline `device_monitor.rs` and
//! phase 1's `File::try_lock` used):
//!
//! - A `zbus::blocking::Connection::system()` connection can
//!   subscribe to the real `PrepareForSleep` signal at
//!   `/org/freedesktop/login1` (`org.freedesktop.login1.Manager`).
//! - Calling `Manager.Inhibit("sleep", ..., "delay")` really succeeds
//!   (no root needed) and really shows up in `systemd-inhibit --list`
//!   while the returned file descriptor is held open, and really
//!   disappears from that list the instant the fd closes -- confirmed
//!   by holding one open for a few seconds from a throwaway probe
//!   binary and checking `systemd-inhibit --list` from another shell
//!   while it ran.
//!
//! What was deliberately never done: actually triggering a real
//! system suspend against this machine. That would suspend the whole
//! box mid-session -- genuinely disruptive, not just to this crate's
//! own testing. Every test in this module drives [`crate::library_handle::Shared`]'s
//! `prepare_for_suspend`/`resume` directly with a synthetic boolean,
//! the same way `device_monitor.rs`'s tests call `apply_event`
//! directly instead of needing a real device removal.
//!
//! # Design
//!
//! [`spawn_power_monitor`] is best-effort, like `device_monitor.rs`'s
//! monitor: if the system bus, `logind`, or the inhibitor call aren't
//! available (a sandboxed/CI environment, a system without
//! `systemd-logind`), it logs and this crate simply proceeds without
//! §5 handling -- `LibraryHandle::open` never fails because of it.
//!
//! The monitor thread holds only a [`std::sync::Weak`] reference to
//! the handle's shared state -- critically, this is what makes
//! **releasing the inhibitor promptly when the handle drops**
//! correct without the thread needing to notice or cooperate at all.
//! The inhibitor's file descriptor lives inside
//! [`crate::library_handle::Shared`] itself (`Arc`-owned, strong-owned
//! only by the `LibraryHandle`); when the handle drops and the
//! `Shared`'s last strong reference goes away, the fd inside is
//! dropped as part of that, releasing the inhibitor immediately --
//! regardless of whether this thread is still blocked waiting on the
//! next D-Bus signal (which, absent an actual system-wide sleep/
//! resume happening, it may be for an unbounded time after the handle
//! drops -- see "disclosed simplifications").
//!
//! # Disclosed simplifications
//!
//! - **WAL checkpoint on suspend -- real, as of issue #260.** §5 step
//!   1's "flushes pending writes, checkpoints WAL" is now real:
//!   `Shared::prepare_for_suspend` calls
//!   `crate::library_handle::checkpoint_wal_best_effort`, which opens
//!   its own short-lived connection to `metadata.db` and each sidecar
//!   database purely to issue `PRAGMA wal_checkpoint(TRUNCATE)` --
//!   `LibraryHandle` doesn't need a connection to any live `Backend`
//!   to do this (SQLite lets *any* connection checkpoint a database
//!   file's WAL, not just the one that wrote to it), which sidesteps
//!   the "multiple independent `Backend` instances can be open on one
//!   library at once" complexity a design needing `LibraryHandle` to
//!   track live `Backend`s would have had. Best-effort: a missing
//!   sidecar or a checkpoint that can't fully `TRUNCATE` (e.g.
//!   something else has an open read transaction) isn't an error --
//!   §5 step 1 is "flush what you can", not a hard precondition for
//!   suspending.
//! - **§5 step 2 blocking-with-timeout semantics -- real, as of issue
//!   #259.** `check_open` (used by every write path) now blocks for
//!   up to 30s waiting for `resume()` while the handle is `Suspended`,
//!   via a `Condvar` paired with `Shared`'s state lock; `set_state`
//!   notifies it on every transition (this thread's own `resume()`
//!   call included, and `device_monitor.rs`'s `Detached` transition
//!   too, automatically -- no per-caller wiring needed, since
//!   `set_state` is the one shared chokepoint every transition already
//!   goes through). `Detached` still fails fast with no blocking when
//!   the handle is already in that state at the time of the call. See
//!   `library_handle.rs`'s module doc for the full design.
//! - **Windows: not implemented**, same disclosed reason as every
//!   other Windows-specific gap in this crate's fault-tolerance work
//!   -- `PowerRegisterSuspendResumeNotification`/`WM_POWERBROADCAST`
//!   can't be compiled or tested on this workspace's Linux-only
//!   toolchain. This whole module is `#[cfg(unix)]`-gated at the
//!   `lib.rs` level.
//! - **The monitor thread may outlive the handle by an unbounded
//!   amount of wall-clock time** if the process keeps running after
//!   the `LibraryHandle` drops and no further system sleep/resume
//!   happens before the process itself exits. This is deliberately
//!   not "fixed" with a receive-timeout poll loop the way
//!   `device_monitor.rs`'s netlink socket uses one: the resource that
//!   actually matters here (the sleep inhibitor) is already released
//!   correctly and promptly via `Shared`'s `Arc` refcount dropping to
//!   zero, independent of this thread's own lifecycle -- the thread
//!   itself, blocked on a D-Bus read, costs one thread stack and one
//!   open socket, which is a low-stakes leak, not a functional bug.

use std::sync::Weak;
use zbus::blocking::Connection;
use zbus::MatchRule;

use crate::library_handle::Shared;

pub(crate) fn spawn_power_monitor(shared: Weak<Shared>) {
    std::thread::Builder::new()
        .name("power-monitor".to_string())
        .spawn(move || {
            if let Err(e) = run(&shared) {
                eprintln!(
                    "sleep/resume monitor not started ({e}) -- \
                     continuing without §5 handling"
                );
            }
        })
        .ok(); // As best-effort as the connection itself failing.
}

fn run(shared: &Weak<Shared>) -> zbus::Result<()> {
    let conn = Connection::system()?;
    acquire_inhibitor(&conn, shared);

    let rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("org.freedesktop.login1.Manager")?
        .member("PrepareForSleep")?
        .path("/org/freedesktop/login1")?
        .build();
    let messages = zbus::blocking::MessageIterator::for_match_rule(rule, &conn, None)?;

    for msg in messages {
        let Some(shared) = shared.upgrade() else {
            return Ok(()); // The handle is gone -- see module doc.
        };
        let Ok(msg) = msg else { continue };
        let Ok(going_to_sleep) = msg.body().deserialize::<bool>() else {
            continue;
        };

        if going_to_sleep {
            shared.prepare_for_suspend();
            shared.set_inhibitor(None); // Let the real suspend proceed.
        } else {
            shared.resume();
            // Re-acquire *before* looping back to wait for the next
            // signal -- must be held continuously between sleep
            // cycles, not just reactively after one starts (see
            // module doc's "why an inhibitor at all").
            acquire_inhibitor(&conn, &std::sync::Arc::downgrade(&shared));
        }
    }
    Ok(())
}

fn acquire_inhibitor(conn: &Connection, shared: &Weak<Shared>) {
    let Some(shared) = shared.upgrade() else {
        return;
    };
    match take_inhibitor(conn) {
        Ok(fd) => shared.set_inhibitor(Some(fd)),
        Err(e) => eprintln!(
            "could not take a sleep inhibitor ({e}) -- suspend-prep is no longer \
             guaranteed to finish before this machine actually suspends"
        ),
    }
}

fn take_inhibitor(conn: &Connection) -> zbus::Result<std::os::fd::OwnedFd> {
    let reply = conn.call_method(
        Some("org.freedesktop.login1"),
        "/org/freedesktop/login1",
        Some("org.freedesktop.login1.Manager"),
        "Inhibit",
        &(
            "sleep",
            "calibre-oxide",
            "flush and release the library writer lock before suspend",
            "delay",
        ),
    )?;
    let fd: zbus::zvariant::OwnedFd = reply.body().deserialize()?;
    Ok(fd.into())
}

#[cfg(test)]
mod tests {
    use crate::library_handle::{shared_for_test, HandleState};
    use std::fs;

    #[test]
    fn prepare_for_suspend_releases_the_lock_and_sets_suspended() {
        let _flock_test_guard = crate::library_handle::flock_test_guard();
        let dir = tempfile::tempdir().unwrap();
        let shared = shared_for_test(dir.path());
        assert_eq!(shared.state(), HandleState::Open);

        shared.prepare_for_suspend();

        assert_eq!(shared.state(), HandleState::Suspended);
        // The lock file was really released -- a fresh open+try_lock
        // from outside this handle must now succeed.
        let lock_path = dir.path().join(".calibre-oxide").join("writer.lock");
        let f = fs::OpenOptions::new().write(true).open(&lock_path).unwrap();
        assert!(f.try_lock().is_ok());
    }

    #[test]
    fn prepare_for_suspend_really_checkpoints_the_wal() {
        // docs/FAULT_TOLERANCE.md §5 step 1 (issue #260): real, not
        // just documented -- a real `metadata.db` (WAL mode as of
        // issue #260's other half) has real pending WAL content from
        // its own schema creation; `PRAGMA wal_checkpoint(TRUNCATE)`
        // truncates the `-wal` file to zero bytes on a full,
        // uncontended checkpoint, which is a strong, direct signal a
        // real checkpoint happened (not just that the call didn't
        // error).
        let _flock_test_guard = crate::library_handle::flock_test_guard();
        let dir = tempfile::tempdir().unwrap();
        let _backend = crate::backend::Backend::new(dir.path()).unwrap();

        let wal_path = dir.path().join("metadata.db-wal");
        assert!(wal_path.exists(), "expected real pending WAL content");
        assert!(fs::metadata(&wal_path).unwrap().len() > 0);

        let shared = shared_for_test(dir.path());
        shared.prepare_for_suspend();

        assert_eq!(
            fs::metadata(&wal_path).unwrap().len(),
            0,
            "a full checkpoint should truncate the WAL file"
        );
    }

    #[test]
    fn resume_returns_to_open_when_nothing_changed() {
        let _flock_test_guard = crate::library_handle::flock_test_guard();
        let dir = tempfile::tempdir().unwrap();
        let shared = shared_for_test(dir.path());
        shared.prepare_for_suspend();
        assert_eq!(shared.state(), HandleState::Suspended);

        shared.resume();

        assert_eq!(shared.state(), HandleState::Open);
    }

    #[test]
    fn resume_reacquires_the_lock_so_a_second_open_is_rejected_again() {
        let _flock_test_guard = crate::library_handle::flock_test_guard();
        let dir = tempfile::tempdir().unwrap();
        let shared = shared_for_test(dir.path());
        shared.prepare_for_suspend();

        shared.resume();

        assert_eq!(shared.state(), HandleState::Open);
        let lock_path = dir.path().join(".calibre-oxide").join("writer.lock");
        let f = fs::OpenOptions::new().write(true).open(&lock_path).unwrap();
        assert!(
            f.try_lock().is_err(),
            "resume should have reacquired the writer lock"
        );
    }

    #[test]
    fn resume_detaches_when_the_persisted_library_id_no_longer_matches() {
        let _flock_test_guard = crate::library_handle::flock_test_guard();
        // Simulates the airport-SSD scenario: something else (or a
        // corrupted version of the same library) is at this path now.
        let dir = tempfile::tempdir().unwrap();
        let shared = shared_for_test(dir.path());
        shared.prepare_for_suspend();

        fs::write(
            dir.path().join(".calibre-oxide").join("library.id"),
            "a-completely-different-library-id",
        )
        .unwrap();

        shared.resume();

        assert_eq!(shared.state(), HandleState::Detached);
    }

    #[test]
    fn resume_detaches_when_library_id_is_missing_entirely() {
        let _flock_test_guard = crate::library_handle::flock_test_guard();
        let dir = tempfile::tempdir().unwrap();
        let shared = shared_for_test(dir.path());
        shared.prepare_for_suspend();

        fs::remove_file(dir.path().join(".calibre-oxide").join("library.id")).unwrap();

        shared.resume();

        assert_eq!(shared.state(), HandleState::Detached);
    }

    /// Counts real `systemd-inhibit --list` rows mentioning
    /// `"calibre-oxide"` -- used as a delta (before/after), not an
    /// absolute check, so this stays correct even if other tests in
    /// this same `cargo test` run are concurrently holding their own
    /// such inhibitors (only `power_monitor.rs`'s own tests and
    /// `library_handle.rs`'s one dedicated public-API test acquire a
    /// real one at all -- see that test's module doc for why the rest
    /// of this crate's test suite deliberately doesn't).
    fn calibre_inhibitor_count() -> usize {
        std::process::Command::new("systemd-inhibit")
            .arg("--list")
            .arg("--no-legend")
            .output()
            .map(|out| {
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .filter(|line| line.contains("calibre-oxide"))
                    .count()
            })
            .unwrap_or(0)
    }

    #[test]
    fn opening_and_dropping_a_real_handle_acquires_and_releases_a_real_sleep_inhibitor() {
        let _flock_test_guard = crate::library_handle::flock_test_guard();
        // The one fully end-to-end test of this module: real zbus
        // connection to this machine's real system bus, a real
        // `Manager.Inhibit` call, a real fd stored on a real `Shared`,
        // released by real `Arc` refcounting when the handle drops.
        // See this module's doc for how the underlying mechanism was
        // verified safe on this exact machine before writing any of
        // this (a throwaway probe, `systemd-inhibit --list` watched
        // from another shell) -- this test exercises the same real
        // path through the actual crate code instead of a probe.
        let before = calibre_inhibitor_count();

        let dir = tempfile::tempdir().unwrap();
        let handle = crate::library_handle::LibraryHandle::open(dir.path()).unwrap();

        // The inhibitor is acquired asynchronously by the background
        // thread (connect to the bus, then call Inhibit) -- poll
        // rather than assume a fixed delay is enough.
        let mut after_open = before;
        for _ in 0..25 {
            after_open = calibre_inhibitor_count();
            if after_open > before {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        assert!(
            after_open > before,
            "expected a real sleep inhibitor to appear within ~5s (before={before}, after={after_open})"
        );

        drop(handle);

        let mut after_drop = after_open;
        for _ in 0..25 {
            after_drop = calibre_inhibitor_count();
            if after_drop < after_open {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        assert!(
            after_drop < after_open,
            "expected the sleep inhibitor to be released promptly after the handle dropped \
             (after_open={after_open}, after_drop={after_drop})"
        );
    }
}
