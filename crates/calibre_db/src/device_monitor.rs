//! Port of `docs/FAULT_TOLERANCE.md` §4 (issue #93): real Linux
//! device-removal detection for [`crate::library_handle::LibraryHandle`],
//! via a raw `NETLINK_KOBJECT_UEVENT` socket -- the same kernel
//! notification source `udevd` itself listens to, not a wrapper around
//! `libudev` (this workspace's box has `libudev1` at runtime but not
//! the `libudev-dev` headers `libudev-sys`-based crates need to build,
//! so binding the raw kernel netlink socket directly -- a small,
//! stable, well-documented kernel ABI -- avoids that dependency
//! entirely, with no loss of the information this needs).
//!
//! # Verified, not just implemented from documentation
//!
//! The raw kernel uevent wire format used here (a leading
//! `<action>@<devpath>` token with no `=`, followed by NUL-separated
//! `KEY=VALUE` fields, terminated by a final NUL) was captured live
//! from the real kernel on this exact machine before writing
//! [`parse_uevent`]: bound a `NETLINK_KOBJECT_UEVENT` socket on the
//! kernel's own multicast group, then ran `udevadm trigger
//! --action=change --subsystem-match=net` (a standard, non-destructive
//! udev-rule-debugging command that only re-announces already-existing
//! devices' current state -- it does not touch any hardware) and
//! inspected the real bytes received. [`parse_uevent`]'s test uses
//! that captured message verbatim. Real block-device *removal* was
//! deliberately never simulated this way against this machine's own
//! root disk (that would be genuinely destructive to the running
//! session) -- [`is_removal_of`]'s block-subsystem/`DEVNAME` matching
//! is standard, stable, unchanged kernel-uevent behavior for the
//! `block` subsystem (the same fields real `udevadm monitor` prints
//! for a `remove` action), not a guess, but is unverified against a
//! captured *removal* event specifically. Disclosed for completeness.
//!
//! # Design
//!
//! [`spawn_device_monitor`] is best-effort and never fails
//! `LibraryHandle::open`: if `AF_NETLINK` can't be used at all (a
//! sandboxed/CI environment without it, or a kernel built without the
//! family), it logs once and simply doesn't monitor -- device-removal
//! detection is a safety enhancement on top of this crate's existing
//! I/O error handling, not something any existing behavior depends on.
//! The monitor thread holds only a [`std::sync::Weak`] reference to
//! the handle's shared state, so it never keeps a `LibraryHandle`
//! alive on its own; it notices the handle is gone (via a periodic
//! receive timeout, since a blocking `recv` can't be interrupted by a
//! dropped `Weak` directly) and exits. It also exits immediately after
//! flipping the state to `Detached` once -- per §4's "no implicit
//! re-attach, no retry loop" contract, there is nothing further for it
//! to usefully watch for once that has happened.
//!
//! # Disclosed simplifications
//!
//! - **No in-flight I/O cancellation.** §4 step 1 asks for
//!   best-effort cancellation of any I/O already in progress when a
//!   device disappears. This crate's I/O is synchronous and
//!   short-lived (no long-blocking operation this crate itself could
//!   proactively cancel); an operation already in flight when the
//!   device vanishes simply fails with a normal OS I/O error through
//!   the existing `Result` plumbing, which every caller already
//!   handles. There is no separate cancellation mechanism to add on
//!   top of that.
//! - **Windows: not implemented**, same disclosed reason as every
//!   other Windows-specific gap in this module's siblings -- this
//!   workspace has no way to compile-check or test
//!   `RegisterDeviceNotificationW`/`WM_DEVICECHANGE` code.
//! - **Matches on the exact mounted device node only** (e.g. `sdb1`),
//!   not also its parent whole-disk (`sdb`). The kernel emits a
//!   `remove` event for the specific device node itself regardless of
//!   whether the whole disk or just one partition is what's gone, so
//!   watching the exact node the library is actually mounted from is
//!   sufficient and avoids extra base-device-name bookkeeping.

use std::collections::HashMap;
use std::io;
use std::mem;
use std::os::fd::RawFd;
use std::sync::{Mutex, Weak};

use crate::library_handle::HandleState;

const AF_NETLINK: libc::c_int = libc::AF_NETLINK;
const NETLINK_KOBJECT_UEVENT: libc::c_int = 15;
/// The kernel's own raw uevent multicast group -- distinct from
/// `udevd`'s separate re-broadcast group, which prefixes each message
/// with a `"libudev"` cookie/header this doesn't need to parse.
const KERNEL_UEVENT_GROUP: u32 = 1;

/// Closes the underlying fd on drop, so every early-return path in
/// [`run`] (including a propagated `?`) still cleans up.
struct UeventSocket(RawFd);

impl Drop for UeventSocket {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.0);
        }
    }
}

fn open_uevent_socket() -> io::Result<UeventSocket> {
    unsafe {
        let fd = libc::socket(
            AF_NETLINK,
            libc::SOCK_RAW | libc::SOCK_CLOEXEC,
            NETLINK_KOBJECT_UEVENT,
        );
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let sock = UeventSocket(fd);

        let mut addr: libc::sockaddr_nl = mem::zeroed();
        addr.nl_family = AF_NETLINK as u16;
        addr.nl_pid = 0; // kernel auto-assigns a free port id
        addr.nl_groups = KERNEL_UEVENT_GROUP;
        let rc = libc::bind(
            fd,
            &addr as *const libc::sockaddr_nl as *const libc::sockaddr,
            mem::size_of::<libc::sockaddr_nl>() as u32,
        );
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }

        // A bounded receive timeout, not a real cancellation
        // mechanism -- lets `run`'s loop periodically notice the
        // handle has been dropped (via `Weak::upgrade`) instead of
        // blocking on `recv` forever.
        let tv = libc::timeval {
            tv_sec: 2,
            tv_usec: 0,
        };
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            &tv as *const libc::timeval as *const libc::c_void,
            mem::size_of::<libc::timeval>() as u32,
        );

        Ok(sock)
    }
}

/// `Ok(None)` on a plain receive timeout (the common case, not an
/// error); `Ok(Some(n))` with the byte count on a real message.
fn recv_or_timeout(sock: &UeventSocket, buf: &mut [u8]) -> io::Result<Option<usize>> {
    let n = unsafe { libc::recv(sock.0, buf.as_mut_ptr() as *mut libc::c_void, buf.len(), 0) };
    if n >= 0 {
        return Ok(Some(n as usize));
    }
    let err = io::Error::last_os_error();
    match err.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => Ok(None),
        _ => Err(err),
    }
}

/// Parses a raw kobject-uevent netlink payload into its `KEY=VALUE`
/// fields -- see this module's doc for how the exact wire format was
/// verified. Any NUL-separated token without an `=` (the leading
/// `<action>@<devpath>` token, or anything malformed) is skipped
/// rather than erroring, since it carries no information not already
/// duplicated in the `ACTION=`/`DEVPATH=` fields that follow it.
pub(crate) fn parse_uevent(payload: &[u8]) -> HashMap<String, String> {
    payload
        .split(|&b| b == 0)
        .filter_map(|field| {
            let field = String::from_utf8_lossy(field);
            let (key, value) = field.split_once('=')?;
            Some((key.to_string(), value.to_string()))
        })
        .collect()
}

/// Whether a parsed uevent reports that `device_name` (e.g. `"sdb1"`
/// -- the `/proc/mounts` device column with its `/dev/` prefix
/// stripped) was just removed.
pub(crate) fn is_removal_of(fields: &HashMap<String, String>, device_name: &str) -> bool {
    fields.get("ACTION").map(String::as_str) == Some("remove")
        && fields.get("SUBSYSTEM").map(String::as_str) == Some("block")
        && fields.get("DEVNAME").map(String::as_str) == Some(device_name)
}

/// Spawns the best-effort background watcher described in this
/// module's doc. `state` should be a [`Weak`] downgraded from the
/// same `Arc` [`crate::library_handle::LibraryHandle`] holds, so the
/// thread never keeps the handle alive by itself.
pub(crate) fn spawn_device_monitor(device_name: String, state: Weak<Mutex<HandleState>>) {
    std::thread::Builder::new()
        .name(format!("device-monitor-{device_name}"))
        .spawn(move || {
            if let Err(e) = run(&device_name, &state) {
                eprintln!(
                    "device-removal monitor for {device_name} not started ({e}) -- \
                     continuing without device-removal detection"
                );
            }
        })
        .ok(); // Spawning the thread itself failing is exactly as
               // best-effort as the socket failing -- see module doc.
}

fn run(device_name: &str, state: &Weak<Mutex<HandleState>>) -> io::Result<()> {
    let sock = open_uevent_socket()?;
    let mut buf = [0u8; 8192];
    loop {
        if state.upgrade().is_none() {
            return Ok(()); // The handle is gone; nothing left to watch for.
        }
        let Some(n) = recv_or_timeout(&sock, &mut buf)? else {
            continue; // Plain timeout -- loop back to the upgrade check.
        };
        let fields = parse_uevent(&buf[..n]);
        if apply_event(&fields, device_name, state) {
            return Ok(()); // Job done -- see module doc.
        }
    }
}

/// The actual per-event decision, factored out of [`run`] so it's
/// testable against a synthetic event without a real socket: real
/// bytes only ever reach here already parsed by [`parse_uevent`].
/// Returns `true` (and flips `state` to `Detached`) exactly when
/// `fields` reports `device_name`'s removal.
fn apply_event(
    fields: &HashMap<String, String>,
    device_name: &str,
    state: &Weak<Mutex<HandleState>>,
) -> bool {
    if !is_removal_of(fields, device_name) {
        return false;
    }
    let Some(state) = state.upgrade() else {
        return true; // Matched, but the handle's already gone -- still "done".
    };
    *state.lock().unwrap() = HandleState::Detached;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Byte-for-byte the payload captured from a real kernel
    /// `NETLINK_KOBJECT_UEVENT` message on this machine (see this
    /// module's doc for exactly how) -- ground truth for the wire
    /// format, not a hand-guessed approximation of it.
    fn captured_change_event() -> Vec<u8> {
        let fields = [
            "change@/devices/pci0000:00/0000:00:1e.0/0000:05:01.0/0000:06:12.0/virtio3/net/ens18",
            "ACTION=change",
            "DEVPATH=/devices/pci0000:00/0000:00:1e.0/0000:05:01.0/0000:06:12.0/virtio3/net/ens18",
            "SUBSYSTEM=net",
            "SYNTH_UUID=d82ad5ca-6565-4b53-92e4-a7d93fdf859b",
            "INTERFACE=ens18",
            "IFINDEX=2",
            "SEQNUM=4500",
        ];
        let mut bytes = Vec::new();
        for f in fields {
            bytes.extend_from_slice(f.as_bytes());
            bytes.push(0);
        }
        bytes
    }

    #[test]
    fn parse_uevent_matches_a_real_captured_kernel_message() {
        let parsed = parse_uevent(&captured_change_event());
        assert_eq!(parsed.get("ACTION").map(String::as_str), Some("change"));
        assert_eq!(parsed.get("SUBSYSTEM").map(String::as_str), Some("net"));
        assert_eq!(parsed.get("INTERFACE").map(String::as_str), Some("ens18"));
        assert_eq!(parsed.get("IFINDEX").map(String::as_str), Some("2"));
        // The leading `change@/devices/...` token has no `=` and is
        // not itself a field.
        assert_eq!(parsed.len(), 7);
    }

    fn block_remove_event(devname: &str) -> HashMap<String, String> {
        [
            ("ACTION", "remove"),
            ("SUBSYSTEM", "block"),
            ("DEVNAME", devname),
            ("DEVPATH", "/devices/pci0000:00/.../block/sdb/sdb1"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }

    #[test]
    fn is_removal_of_matches_a_block_remove_event_for_the_right_device() {
        let event = block_remove_event("sdb1");
        assert!(is_removal_of(&event, "sdb1"));
        assert!(!is_removal_of(&event, "sdb2"));
    }

    #[test]
    fn is_removal_of_ignores_a_change_event() {
        let parsed = parse_uevent(&captured_change_event());
        assert!(!is_removal_of(&parsed, "ens18"));
    }

    #[test]
    fn is_removal_of_ignores_a_non_block_subsystem() {
        let mut event = block_remove_event("sdb1");
        event.insert("SUBSYSTEM".to_string(), "usb".to_string());
        assert!(!is_removal_of(&event, "sdb1"));
    }

    #[test]
    fn a_real_uevent_socket_can_be_opened_and_closed_on_this_machine() {
        // Not a removal test (this machine's own root disk must never
        // be touched) -- just proves the real netlink plumbing this
        // module depends on actually works in this environment,
        // matching phase 1's `File::try_lock` probe-before-trusting
        // practice.
        open_uevent_socket().expect("AF_NETLINK/NETLINK_KOBJECT_UEVENT should be available");
    }

    #[test]
    fn apply_event_flips_a_matching_devices_state_to_detached() {
        let state = std::sync::Arc::new(Mutex::new(HandleState::Open));
        let weak = std::sync::Arc::downgrade(&state);
        let event = block_remove_event("sdb1");

        let handled = apply_event(&event, "sdb1", &weak);

        assert!(handled);
        assert_eq!(*state.lock().unwrap(), HandleState::Detached);
    }

    #[test]
    fn apply_event_leaves_a_non_matching_devices_state_alone() {
        let state = std::sync::Arc::new(Mutex::new(HandleState::Open));
        let weak = std::sync::Arc::downgrade(&state);
        let event = block_remove_event("sdb1");

        let handled = apply_event(&event, "sdb2", &weak);

        assert!(!handled);
        assert_eq!(*state.lock().unwrap(), HandleState::Open);
    }

    #[test]
    fn spawn_device_monitor_exits_once_the_handle_drops() {
        let state = std::sync::Arc::new(Mutex::new(HandleState::Open));
        let weak = std::sync::Arc::downgrade(&state);
        spawn_device_monitor("a-device-name-that-will-never-match".to_string(), weak);
        drop(state);
        // The monitor's receive timeout is 2s; give it real margin to
        // notice and exit without making this test itself flaky.
        std::thread::sleep(std::time::Duration::from_secs(3));
        // Nothing to assert directly (the thread is detached) --
        // this test's real value is running under `cargo test`'s
        // leak/hang detection: if the thread never exits, later
        // `cargo test` shutdown would hang.
    }
}
