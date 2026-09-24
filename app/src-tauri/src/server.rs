//! Spawns and manages the real `calibre_srv` process this app's
//! window is a native shell around, and locates the two things it
//! needs to find on disk: the compiled `calibre_srv` binary and the
//! built `web/dist` frontend it serves.
//!
//! # Disclosed narrowing
//!
//! Real Tauri "sidecar" bundling (`bundle.externalBin` +
//! `tauri-plugin-shell`, with the platform-specific target-triple
//! binary naming that mechanism requires) isn't set up here -- this
//! spawns `calibre_srv` via a plain [`std::process::Command`], looked
//! up either as a bundled resource (a packaged app) or a
//! `CARGO_MANIFEST_DIR`-relative dev build (`cargo tauri dev`/a local
//! debug build). Real, sufficient for this app to actually run today;
//! proper cross-platform sidecar packaging for end-user distribution
//! is real, separate, follow-up work.

use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

/// Binds to an OS-assigned free port, then releases it immediately so
/// `calibre_srv` (a separate process) can bind it instead. Real, if
/// technically racy against another process grabbing the same port in
/// between -- an acceptable, standard "find a free port" technique for
/// a local desktop app talking only to its own spawned backend.
pub fn find_free_port() -> io::Result<u16> {
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))?;
    Ok(listener.local_addr()?.port())
}

fn dev_workspace_root() -> PathBuf {
    // `app/src-tauri` -> repo root.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Locates the compiled `calibre_srv` binary: a bundled resource
/// (`resources/calibre_srv`, a real packaged app) if present, else the
/// dev workspace's own `target/debug/calibre_srv` (or `release/`, for
/// a local release build run outside `tauri dev`).
pub fn resolve_calibre_srv_binary(app: &AppHandle) -> io::Result<PathBuf> {
    let exe_name = if cfg!(windows) { "calibre_srv.exe" } else { "calibre_srv" };

    if let Ok(resource_dir) = app.path().resource_dir() {
        let candidate = resource_dir.join(exe_name);
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    let root = dev_workspace_root();
    for profile in ["debug", "release"] {
        let candidate = root.join("target").join(profile).join(exe_name);
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(io::Error::new(io::ErrorKind::NotFound, "calibre_srv binary not found -- run `cargo build --workspace` first"))
}

/// Locates the built `web/dist` frontend the same way: bundled
/// resource (`resources/web-dist/`) first, else the dev workspace's
/// own `web/dist`.
pub fn resolve_web_dist(app: &AppHandle) -> io::Result<PathBuf> {
    if let Ok(resource_dir) = app.path().resource_dir() {
        let candidate = resource_dir.join("web-dist");
        if candidate.join("index.html").exists() {
            return Ok(candidate);
        }
    }

    let candidate = dev_workspace_root().join("web").join("dist");
    if candidate.join("index.html").exists() {
        return Ok(candidate);
    }
    Err(io::Error::new(io::ErrorKind::NotFound, "web/dist not found -- run `npm run build` in web/ first"))
}

/// Spawns `calibre_srv <library_path> --static-dir <static_dir> --port
/// <port>` -- real auth stays off (`calibre_srv`'s own default with no
/// `--add-user` ever called): a locally-spawned server that only this
/// app's own window ever talks to has nothing to authenticate against.
pub fn spawn(bin: &Path, library_path: &Path, static_dir: &Path, port: u16) -> io::Result<Child> {
    let mut cmd = Command::new(bin);
    cmd.arg(library_path).arg("--static-dir").arg(static_dir).arg("--port").arg(port.to_string());
    no_console_window(&mut cmd);
    cmd.spawn()
}

/// Keeps Windows from opening a console window for the server.
///
/// `calibre_srv` is a console-subsystem binary, so launching it from a
/// GUI app hands it a fresh console -- a second window that sits behind
/// the app announcing "content server listening on ...". The app itself
/// never had this problem (`main.rs` sets `windows_subsystem`); the
/// child is a separate process and needs telling separately.
///
/// `CREATE_NO_WINDOW` is 0x0800_0000. Spelled out rather than pulled
/// from `windows-sys` because this crate does not otherwise depend on
/// it, and one documented constant is a smaller thing to own than a
/// dependency.
#[cfg(windows)]
fn no_console_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn no_console_window(_cmd: &mut Command) {}

/// Polls a real TCP connect to `127.0.0.1:<port>` until it succeeds or
/// `timeout` elapses. A successful connect is a real readiness signal
/// here (not just "the process exists"): `calibre_srv`'s own startup
/// only binds the listener once its router/state setup has already
/// completed, so nothing accepts a connection before it's actually
/// ready to serve requests.
pub fn wait_until_ready(port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    while Instant::now() < deadline {
        if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}
