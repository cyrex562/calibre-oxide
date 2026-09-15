//! Real, black-box end-to-end test helpers for calibre-oxide.
//!
//! Unlike the unit/integration tests inside each crate (which call
//! Rust functions directly), everything here drives the actual
//! compiled binaries the way a real user or the real desktop app
//! would -- a real subprocess, real stdin/stdout/HTTP, real files on
//! disk. This is deliberately the layer that would have caught the
//! `calibredb` binary not existing / `main_dispatch`'s `"add"` arm
//! being a hardcoded stub (see #704): both were invisible to every
//! in-process unit test, because nothing exercised the actual
//! dispatcher/binary boundary.
//!
//! Not a workspace member other crates depend on -- `[dependencies]`
//! only pulls in what building real test libraries needs
//! (`calibre_db`); everything else (spawning the real binaries) is
//! plain `std::process`.

use std::io::BufRead;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Resolves a compiled workspace binary by name, relative to this
/// crate's own `CARGO_MANIFEST_DIR` (`e2e/` -> workspace root ->
/// `target/<profile>/<name>`). Tries `debug` then `release` so this
/// works whichever profile `cargo build`/`cargo test` was last run
/// with for that binary.
pub fn oxide_bin(name: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let exe_name = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    for profile in ["debug", "release"] {
        let candidate = root.join("target").join(profile).join(&exe_name);
        if candidate.exists() {
            return candidate;
        }
    }
    panic!("{name} not found in target/debug or target/release -- run `cargo build --workspace` first");
}

/// A real, on-disk, empty calibre library (a real `metadata.db` via
/// `calibre_db::Library::create`) that outlives as long as this
/// struct does, then cleans itself up (`tempfile::TempDir`'s own
/// `Drop`).
pub struct TestLibrary {
    dir: tempfile::TempDir,
}

impl TestLibrary {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("failed to create a temp dir for a test library");
        calibre_db::Library::create(dir.path().to_path_buf()).expect("failed to create a real test library");
        Self { dir }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }
}

/// Kills and reaps the wrapped child on drop, so a panicking assertion
/// in a test body never leaks a running `calibre_srv`/worker process.
pub struct ChildGuard(pub Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Binds an OS-assigned free port, then releases it -- the same
/// find-a-free-port technique `app/src-tauri/src/server.rs` uses for
/// the real desktop app, reused here so tests don't collide with a
/// developer's own `calibre_srv` on the default port.
pub fn find_free_port() -> u16 {
    let listener = std::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).expect("failed to bind a free port");
    listener.local_addr().unwrap().port()
}

/// Spawns the real, compiled `calibre_srv` binary against `library`,
/// with `--static-dir static_dir --port port` -- the exact same
/// shape `app/src-tauri/src/server.rs::spawn` uses, so this is really
/// testing the same startup path the desktop app takes, not a
/// parallel one.
pub fn spawn_calibre_srv(library: &Path, static_dir: &Path, port: u16) -> ChildGuard {
    let child = Command::new(oxide_bin("calibre_srv"))
        .arg(library)
        .arg("--static-dir")
        .arg(static_dir)
        .arg("--port")
        .arg(port.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn calibre_srv");
    ChildGuard(child)
}

/// Polls a real TCP connect until `calibre_srv` (or anything else
/// listening on `port`) is actually accepting connections.
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

/// Locates a real, installed upstream `calibredb` on `PATH` (e.g. via
/// `apt install calibre`) distinct from this port's own compiled
/// `calibredb` (resolved separately via [`oxide_bin`], never via
/// `PATH`) -- `None` on a box without real calibre installed, so
/// live-comparison tests can skip gracefully rather than fail.
pub fn upstream_calibredb() -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    let exe_name = if cfg!(windows) { "calibredb.exe" } else { "calibredb" };
    std::env::split_paths(&path_var).map(|dir| dir.join(exe_name)).find(|candidate| candidate.is_file())
}

/// Runs the real `calibre_scraper_worker` binary's JSON-lines
/// protocol (see that crate's own module doc) to fetch `url` through
/// a real, JS-capable OS webview -- the same stack `app/src-tauri`
/// itself uses -- and returns the loaded document's outer HTML.
///
/// Needs a real (or virtual, e.g. `xvfb-run`) display; returns `None`
/// rather than panicking if the worker can't even start (no display
/// available), so a CI box with no Xvfb configured skips this check
/// instead of failing outright.
pub fn webview_fetch(url: &str, timeout: Duration) -> Option<String> {
    use std::io::Write;

    let mut child = ChildGuard(
        Command::new(oxide_bin("calibre_scraper_worker")).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?,
    );

    let request = serde_json::json!({
        "action": "fetch",
        "id": 1,
        "url": url,
        "user_agent": "",
        "headers": [],
        "timeout_ms": timeout.as_millis() as u64,
    });
    writeln!(child.0.stdin.as_mut()?, "{request}").ok()?;

    let mut line = String::new();
    std::io::BufReader::new(child.0.stdout.take()?).read_line(&mut line).ok()?;

    let parsed: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    parsed.get("html").and_then(|h| h.as_str()).map(|s| s.to_string())
}
