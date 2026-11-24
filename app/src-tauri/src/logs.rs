//! Where the app and its server write what happened, and how a failed
//! start is reported to the person looking at the splash.
//!
//! The GUI build has no console, so before this existed a server that
//! failed to start left nothing at all: no window, no message, no file.
//!
//! * `server.log` -- the spawned `lexmechanic_srv`'s stdout and stderr, for the
//!   current run. The run before it is kept as `server.prev.log`, so a
//!   crash followed by a relaunch does not destroy the evidence.
//! * `app.log` -- the app's own errors, appended to.
//!
//! Both live in Tauri's `app_log_dir` (on Windows
//! `%APPDATA%\com.cyrex.lexmechanic\logs`).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Manager};

pub const SERVER_LOG: &str = "server.log";
pub const SERVER_PREV_LOG: &str = "server.prev.log";
pub const APP_LOG: &str = "app.log";

/// How many trailing lines of `server.log` the splash shows.
pub const TAIL_LINES: usize = 20;

/// The log directory; falls back to a folder under the temp dir if Tauri
/// cannot say, so logging never becomes a reason not to start.
pub fn log_dir(app: &AppHandle) -> PathBuf {
    app.path().app_log_dir().unwrap_or_else(|_| std::env::temp_dir().join("lexmechanic-logs"))
}

/// Starts a fresh `server.log`, keeping the previous one as
/// `server.prev.log`, and returns the new file open for writing.
pub fn rotate_server_log(dir: &Path) -> io::Result<File> {
    fs::create_dir_all(dir)?;
    let current = dir.join(SERVER_LOG);
    if current.exists() {
        let prev = dir.join(SERVER_PREV_LOG);
        // Windows `rename` will not replace an existing file.
        let _ = fs::remove_file(&prev);
        fs::rename(&current, &prev)?;
    }
    File::create(current)
}

/// Last `n` lines of a file (reads only its final 64 KiB), or empty if it
/// cannot be read. Lossy UTF-8: a log may hold anything.
pub fn tail_lines(path: &Path, n: usize) -> Vec<String> {
    let Ok(mut f) = File::open(path) else {
        return Vec::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(64 * 1024);
    let mut buf = Vec::new();
    if f.seek(SeekFrom::Start(start)).is_err() || f.read_to_end(&mut buf).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<&str> = text.lines().collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0); // probably cut mid-line
    }
    let from = lines.len().saturating_sub(n);
    lines[from..].iter().map(|l| l.trim_end().to_string()).collect()
}

/// `YYYY-MM-DD HH:MM:SS` (UTC) for a unix time, without a date crate.
fn utc_stamp(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let rem = unix_secs % 86_400;
    // Civil-from-days (H. Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Appends one line to `app.log`. Best effort: failing to log must never
/// take the app down with it.
pub fn append_app_log(dir: &Path, message: &str) {
    let _ = fs::create_dir_all(dir);
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(dir.join(APP_LOG)) {
        let _ = writeln!(f, "{} UTC  {message}", utc_stamp(secs));
    }
    eprintln!("{message}");
}

/// What the splash shows when the library could not be opened.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LaunchFailure {
    pub message: String,
    /// The last lines of `server.log`; empty if the server never got far
    /// enough to write any (or was never started).
    pub log_tail: Vec<String>,
    pub log_dir: String,
}

impl LaunchFailure {
    pub fn new(message: String, dir: &Path) -> Self {
        LaunchFailure { message, log_tail: tail_lines(&dir.join(SERVER_LOG), TAIL_LINES), log_dir: dir.to_string_lossy().into_owned() }
    }
}

/// The most recent failed start, until a later start succeeds.
#[derive(Default)]
pub struct LaunchFailureState(pub Mutex<Option<LaunchFailure>>);

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn the_previous_run_is_kept_as_prev() {
        let d = tmp();
        writeln!(rotate_server_log(d.path()).unwrap(), "first run").unwrap();
        writeln!(rotate_server_log(d.path()).unwrap(), "second run").unwrap();
        assert_eq!(fs::read_to_string(d.path().join(SERVER_LOG)).unwrap(), "second run\n");
        assert_eq!(fs::read_to_string(d.path().join(SERVER_PREV_LOG)).unwrap(), "first run\n");
        // A third run replaces prev rather than failing on the existing file.
        writeln!(rotate_server_log(d.path()).unwrap(), "third run").unwrap();
        assert_eq!(fs::read_to_string(d.path().join(SERVER_PREV_LOG)).unwrap(), "second run\n");
    }

    #[test]
    fn the_log_dir_is_created_if_missing() {
        let d = tmp();
        let nested = d.path().join("a").join("b");
        rotate_server_log(&nested).unwrap();
        assert!(nested.join(SERVER_LOG).is_file());
    }

    #[test]
    fn tail_returns_the_last_lines_in_order() {
        let d = tmp();
        let p = d.path().join("x.log");
        fs::write(&p, (1..=50).map(|i| format!("line {i}\n")).collect::<String>()).unwrap();
        let t = tail_lines(&p, 3);
        assert_eq!(t, vec!["line 48", "line 49", "line 50"]);
        assert_eq!(tail_lines(&p, 500).len(), 50);
    }

    #[test]
    fn tail_of_a_missing_or_empty_file_is_empty() {
        let d = tmp();
        assert!(tail_lines(&d.path().join("nope.log"), 5).is_empty());
        fs::write(d.path().join("e.log"), "").unwrap();
        assert!(tail_lines(&d.path().join("e.log"), 5).is_empty());
    }

    #[test]
    fn tail_of_a_huge_file_drops_the_cut_first_line() {
        let d = tmp();
        let p = d.path().join("big.log");
        fs::write(&p, (0..20_000).map(|i| format!("entry number {i}\n")).collect::<String>()).unwrap();
        let t = tail_lines(&p, 5);
        assert_eq!(t.last().unwrap(), "entry number 19999");
        assert_eq!(t.len(), 5);
    }

    #[test]
    fn tail_survives_bytes_that_are_not_utf8() {
        let d = tmp();
        let p = d.path().join("bin.log");
        fs::write(&p, b"ok\n\xff\xfe broken\n").unwrap();
        assert_eq!(tail_lines(&p, 5).len(), 2);
    }

    #[test]
    fn utc_stamp_is_correct() {
        assert_eq!(utc_stamp(0), "1970-01-01 00:00:00");
        assert_eq!(utc_stamp(951_782_400 + 3661), "2000-02-29 01:01:01");
        assert_eq!(utc_stamp(1_700_000_000), "2023-11-14 22:13:20");
    }

    #[test]
    fn app_log_appends() {
        let d = tmp();
        append_app_log(d.path(), "one");
        append_app_log(d.path(), "two");
        let text = fs::read_to_string(d.path().join(APP_LOG)).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert!(text.lines().next().unwrap().ends_with("UTC  one"));
    }

    #[test]
    fn a_failure_carries_the_server_log_tail() {
        let d = tmp();
        fs::write(d.path().join(SERVER_LOG), "boom\n").unwrap();
        let f = LaunchFailure::new("did not start".into(), d.path());
        assert_eq!(f.log_tail, vec!["boom"]);
    }
}
