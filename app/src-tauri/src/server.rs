//! Spawns and manages the real `lexmechanic_srv` process this app's
//! window is a native shell around, and locates the two things it
//! needs to find on disk: the compiled `lexmechanic_srv` binary and the
//! built `web/dist` frontend it serves.
//!
//! # Disclosed narrowing
//!
//! Real Tauri "sidecar" bundling (`bundle.externalBin` +
//! `tauri-plugin-shell`, with the platform-specific target-triple
//! binary naming that mechanism requires) isn't set up here -- this
//! spawns `lexmechanic_srv` via a plain [`std::process::Command`], looked
//! up either as a bundled resource (a packaged app) or a
//! `CARGO_MANIFEST_DIR`-relative dev build (`cargo tauri dev`/a local
//! debug build). Real, sufficient for this app to actually run today;
//! proper cross-platform sidecar packaging for end-user distribution
//! is real, separate, follow-up work.

use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

/// Binds to an OS-assigned free port, then releases it immediately so
/// `lexmechanic_srv` (a separate process) can bind it instead. Real, if
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

/// Locates the compiled `lexmechanic_srv` binary: a bundled resource
/// (`resources/lexmechanic_srv`, a real packaged app) if present, else the
/// dev workspace's own `target/debug/lexmechanic_srv` (or `release/`, for
/// a local release build run outside `tauri dev`).
pub fn resolve_calibre_srv_binary(app: &AppHandle) -> io::Result<PathBuf> {
    let exe_name = if cfg!(windows) { "lexmechanic_srv.exe" } else { "lexmechanic_srv" };

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
    Err(io::Error::new(io::ErrorKind::NotFound, "lexmechanic_srv binary not found -- run `cargo build --workspace` first"))
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

/// Locates the PDFium library the server renders PDF pages with.
///
/// Optional, unlike the two above: without it the server still runs
/// and PDFs still import, they just get no cover rendered from page 1.
/// So this returns `Option` rather than `io::Result` -- "not installed"
/// is a normal state, not a failure to report.
///
/// The server would find a copy sitting next to its own binary on its
/// own (see `rasterize::candidate_library_paths`), but in a packaged
/// app the resource directory is not where the binary is on every
/// platform -- a Linux `.deb` puts resources under `/usr/lib/<app>/`.
/// Resolving it here and passing it explicitly means one answer on all
/// three platforms.
pub fn resolve_pdfium(app: &AppHandle) -> Option<PathBuf> {
    let lib_name = if cfg!(windows) {
        "pdfium.dll"
    } else if cfg!(target_os = "macos") {
        "libpdfium.dylib"
    } else {
        "libpdfium.so"
    };

    if let Ok(resource_dir) = app.path().resource_dir() {
        let candidate = resource_dir.join(lib_name);
        if candidate.exists() {
            return Some(candidate);
        }
    }

    let root = dev_workspace_root();
    for profile in ["debug", "release"] {
        let candidate = root.join("target").join(profile).join(lib_name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// Spawns `lexmechanic_srv <library_path> --static-dir <static_dir> --port
/// <port>` -- real auth stays off (`lexmechanic_srv`'s own default with no
/// `--add-user` ever called): a locally-spawned server that only this
/// app's own window ever talks to has nothing to authenticate against.
/// `also_serve` are the user's other known libraries, passed as
/// `--library` so that a `library_id` in a URL means something (#959).
/// Without them the server hosts one library and copy-to-library has
/// nowhere to copy to, however many libraries the user has opened before.
///
/// The currently-open library is deliberately not repeated in this list:
/// it is the positional argument, and the broker would skip a duplicate
/// path anyway.
pub fn spawn(bin: &Path, library_path: &Path, static_dir: &Path, port: u16, pdfium: Option<&Path>, also_serve: &[PathBuf], log: Option<std::fs::File>) -> io::Result<Child> {
    let mut cmd = Command::new(bin);
    // Loopback only: the desktop app's server is private to this machine.
    // The server's own default (0.0.0.0, as calibre's) would expose the
    // library to the LAN with no login and trigger a firewall prompt.
    cmd.arg(library_path).arg("--static-dir").arg(static_dir).arg("--port").arg(port.to_string()).arg("--listen-on").arg("127.0.0.1");
    for other in also_serve {
        // A library the user has since deleted or unplugged must not stop
        // the app from starting; `LibraryBroker::new` skips a path with no
        // `metadata.db`, but a path that is gone entirely is cheaper to
        // drop here than to have the server decide about.
        if other != library_path && other.join("metadata.db").is_file() {
            cmd.arg("--library").arg(other);
        }
    }
    if let Some(pdfium) = pdfium {
        // Read by `lexmechanic_ebooks::pdf::rasterize`. An environment
        // variable rather than a flag because every binary in the
        // workspace that can render a PDF reads the same one --
        // `calibredb` included -- and only one of them has a CLI this
        // app constructs.
        cmd.env(lexmechanic_ebooks::pdf::rasterize::LIBRARY_PATH_VAR, pdfium);
    }
    // With no console (see below) stdout/stderr go nowhere, and a server
    // that fails to start would die silently. Both streams go to the same
    // file so the order of lines is the order things happened.
    cmd.stdin(Stdio::null());
    if let Some(log) = log {
        cmd.stderr(Stdio::from(log.try_clone()?)).stdout(Stdio::from(log));
    }
    no_console_window(&mut cmd);
    cmd.spawn()
}

/// Keeps Windows from opening a console window for the server.
///
/// `lexmechanic_srv` is a console-subsystem binary, so launching it from a
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

/// How long to wait for the server to accept connections: 60 s, or
/// `LEXMECHANIC_READY_TIMEOUT_SECS`.
pub fn ready_timeout() -> Duration {
    parse_ready_timeout(lexmechanic_utils::env::var("LEXMECHANIC_READY_TIMEOUT_SECS").ok().as_deref())
}

fn parse_ready_timeout(raw: Option<&str>) -> Duration {
    Duration::from_secs(raw.and_then(|v| v.trim().parse::<u64>().ok()).filter(|s| *s > 0).unwrap_or(60))
}

#[cfg(test)]
mod ready_timeout_tests {
    use super::*;

    #[test]
    fn a_large_library_gets_a_minute_by_default() {
        assert_eq!(parse_ready_timeout(None), Duration::from_secs(60));
        assert_eq!(parse_ready_timeout(Some("garbage")), Duration::from_secs(60));
        assert_eq!(parse_ready_timeout(Some("120")), Duration::from_secs(120));
    }
}

/// How waiting for the server ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Readiness {
    Ready,
    /// The process ended before it ever accepted a connection; carries a
    /// description of how ("exit code: 1").
    Exited(String),
    TimedOut,
}

/// Polls a real TCP connect to `127.0.0.1:<port>` until it succeeds,
/// `exited` reports the process is gone, or `timeout` elapses. A
/// successful connect is a real readiness signal here (not just "the
/// process exists"): `lexmechanic_srv`'s own startup only binds the listener
/// once its router/state setup has already completed, so nothing accepts
/// a connection before it's actually ready to serve requests.
///
/// `exited` is what makes a crash on startup (a missing DLL, a corrupt
/// library) fail in a moment instead of after the full timeout.
pub fn wait_until_ready(port: u16, timeout: Duration, mut exited: impl FnMut() -> Option<String>) -> Readiness {
    let deadline = Instant::now() + timeout;
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    while Instant::now() < deadline {
        if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
            return Readiness::Ready;
        }
        if let Some(how) = exited() {
            return Readiness::Exited(how);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Readiness::TimedOut
}

#[cfg(test)]
mod readiness_tests {
    use super::*;

    #[test]
    fn ready_when_something_listens() {
        let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        assert_eq!(wait_until_ready(port, Duration::from_secs(5), || None), Readiness::Ready);
    }

    #[test]
    fn a_dead_process_fails_fast_not_after_the_timeout() {
        let port = find_free_port().unwrap();
        let started = Instant::now();
        let r = wait_until_ready(port, Duration::from_secs(30), || Some("exit code: 1".into()));
        assert_eq!(r, Readiness::Exited("exit code: 1".into()));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn times_out_when_nothing_listens_and_nothing_died() {
        let port = find_free_port().unwrap();
        assert_eq!(wait_until_ready(port, Duration::from_millis(300), || None), Readiness::TimedOut);
    }
}
